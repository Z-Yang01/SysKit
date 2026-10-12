//! 密码生成器：纯函数核心 + 系统 CSPRNG 入口（vault「生成」按钮的后端）。
//!
//! 三条硬保证（各有一条对应测试锁死，变异验证见提交说明）：
//! - **随机性**只有一条来源：`w32::random_bytes`（BCryptGenRandom 系统首选
//!   CSPRNG）。绝不允许时间戳/自建 PRNG 当种子——密码的不可预测性完全由
//!   随机源决定；测试通过注入确定性字节序列驱动核心，生产路径零分叉。
//! - **均匀性**靠拒绝采样：字节直接 `% n` 有模偏差（256 不整除 n 时头部
//!   取值概率偏高），先丢弃 `>= bound` 的字节再取模，每个索引严格等概率。
//! - **每类至少一个**由构造保证：先从每个启用类各取 1 个必选字符，剩余位
//!   从合并字母表取，最后 Fisher–Yates 洗牌——洗牌只重排不打删，保底恒成立。
//!   不可满足（长度 < 启用类数）在入口报参数错误，绝不静默截断保底。

/// 默认长度（前端滑杆初始值与此对齐）。
pub const DEFAULT_LENGTH: usize = 16;
/// 长度合法区间（闭区间）：短于 8 抗不住在线猜测，长于 64 多数站点不收。
pub const MIN_LENGTH: usize = 8;
pub const MAX_LENGTH: usize = 64;

/// 四类字符集（排除前）。全开 + 不排除 = 94 个，经典键盘可用集。
/// **与前端 vault.ts 的 PWGEN_* 常量逐字对齐**，由 tests/vault.test.ts 的
/// parity 守卫读本文件字面量比对，任何一侧单独改动都会红。
const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const DIGITS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*()-_=+[]{};:,.<>/?~\\|'\"`";

/// 排除易混淆选项要剔除的字符（跨类共用）：0/O、1/l/I、竖线、反引号、单双引号
/// ——手抄/客服口述时最易看错的一批。每类剔除后仍非空（24/25/8/28），
/// 「至少一类」约束不会被排除选项破坏。
pub const AMBIGUOUS: &[u8] = b"0O1lI|`'\"";

/// 生成选项。四个类至少启用一个；长度在 [`MIN_LENGTH`]..=[`MAX_LENGTH`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenOptions {
    pub length: usize,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
    pub symbols: bool,
    pub exclude_ambiguous: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions {
            length: DEFAULT_LENGTH,
            upper: true,
            lower: true,
            digits: true,
            symbols: true,
            exclude_ambiguous: false,
        }
    }
}

/// 参数校验（纯函数，独立成便于单测的形态）：长度区间、至少一类、
/// 长度足以容纳每类一个的保底。三个分支各有专属错误文案。
fn validate(length: usize, n_classes: usize) -> Result<(), String> {
    if n_classes == 0 {
        return Err("至少启用一类字符（大写/小写/数字/符号）".into());
    }
    // 先于长度区间判：长度再离谱，「不够每类一个」也是更本质的不可满足
    if length < n_classes {
        return Err(format!(
            "长度 {length} 小于启用的字符类数 {n_classes}，无法保证每类至少出现一个"
        ));
    }
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&length) {
        return Err(format!(
            "长度须在 {MIN_LENGTH}~{MAX_LENGTH} 之间，收到 {length}"
        ));
    }
    Ok(())
}

/// 启用类的过滤后字符集（大写/小写/数字/符号固定顺序），排除易混淆按选项生效。
fn enabled_classes(o: &GenOptions) -> Vec<Vec<u8>> {
    let raw: [&[u8]; 4] = [UPPER, LOWER, DIGITS, SYMBOLS];
    let on = [o.upper, o.lower, o.digits, o.symbols];
    raw.iter()
        .zip(on.iter())
        .filter(|(_, &enable)| enable)
        .map(|(&cs, _)| {
            if o.exclude_ambiguous {
                cs.iter()
                    .copied()
                    .filter(|c| !AMBIGUOUS.contains(c))
                    .collect()
            } else {
                cs.to_vec()
            }
        })
        .collect()
}

/// 随机字节供给：按需向随机源要 64 字节一批，业务侧只管取。
/// 批量取而不是逐字节取：省掉每个字符一次 FFI 的开销，语义不变。
struct RngBytes<'a> {
    rng: &'a mut dyn FnMut(usize) -> Vec<u8>,
    buf: Vec<u8>,
    pos: usize,
}

impl RngBytes<'_> {
    fn next_byte(&mut self) -> u8 {
        if self.pos >= self.buf.len() {
            self.buf = (self.rng)(64);
            self.pos = 0;
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        b
    }
}

/// 从随机字节流取 [0, n) 均匀索引（拒绝采样，无模偏差）。
///
/// bound = 256 - (256 % n) 是 256 以内最大的 n 的整倍数：落在
/// `[bound, 256)` 的字节会让 `b % n` 的头部取值多摊一份概率，必须整段丢弃；
/// 接受区内 `b % n` 对每个索引恰好等概率。n ≤ 94 < 256，单字节足够。
fn pick_index(n: usize, src: &mut RngBytes) -> usize {
    let n32 = n as u32;
    let bound = 256 - (256 % n32);
    loop {
        let b = u32::from(src.next_byte());
        if b < bound {
            return (b % n32) as usize;
        }
    }
}

/// 生成核心（纯函数：随机源经参数注入，单测用确定性序列锁行为）。
pub fn generate_with(
    opts: GenOptions,
    rng: &mut dyn FnMut(usize) -> Vec<u8>,
) -> Result<String, String> {
    let classes = enabled_classes(&opts);
    validate(opts.length, classes.len())?;
    let full: Vec<u8> = classes.concat();
    let mut src = RngBytes {
        rng,
        buf: Vec::new(),
        pos: 0,
    };
    let mut chars: Vec<u8> = Vec::with_capacity(opts.length);
    // 每类一个必选字符：类内均匀取（类内字母表本身已被排除选项过滤）
    for c in &classes {
        chars.push(c[pick_index(c.len(), &mut src)]);
    }
    // 其余位从合并字母表取
    while chars.len() < opts.length {
        chars.push(full[pick_index(full.len(), &mut src)]);
    }
    // Fisher–Yates 洗牌：必选字符只换位置不消失，「每类至少一个」恒成立；
    // 索引同样走 pick_index，不引入第二处采样实现
    for i in (1..chars.len()).rev() {
        let j = pick_index(i + 1, &mut src);
        chars.swap(i, j);
    }
    // 字符集全是 ASCII 字节，as char 无损
    Ok(chars.into_iter().map(char::from).collect())
}

/// 真实入口：随机源 = 系统 CSPRNG（BCryptGenRandom，见 w32::random_bytes）。
pub fn generate(opts: GenOptions) -> Result<String, String> {
    generate_with(opts, &mut |n| crate::w32::random_bytes(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SplitMix64 确定性伪随机（仅测试注入用；生产路径唯一入口是 generate）。
    /// u64 状态：8 位小状态 LCG 会让相邻种子快速汇入同一循环（实测 42/43
    /// 产出同一密码），做「不同种子必不同」断言前先换大状态发生器。
    fn splitmix_rng(seed: u64) -> impl FnMut(usize) -> Vec<u8> {
        let state = std::cell::Cell::new(seed);
        move |n: usize| {
            (0..n)
                .map(|_| {
                    let s = state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
                    state.set(s);
                    let mut z = s;
                    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                    (z ^ (z >> 31)) as u8
                })
                .collect()
        }
    }

    fn opts(length: usize, upper: bool, lower: bool, digits: bool, symbols: bool) -> GenOptions {
        GenOptions {
            length,
            upper,
            lower,
            digits,
            symbols,
            exclude_ambiguous: false,
        }
    }

    #[test]
    fn validate_rejects_length_out_of_range() {
        assert!(validate(7, 4).is_err(), "低于下限必须报参数错误");
        assert!(validate(0, 4).is_err());
        assert!(validate(65, 4).is_err(), "高于上限必须报参数错误");
        assert!(validate(MIN_LENGTH, 4).is_ok());
        assert!(validate(MAX_LENGTH, 4).is_ok());
        assert!(validate(DEFAULT_LENGTH, 1).is_ok());
    }

    #[test]
    fn validate_rejects_zero_classes_and_length_below_class_count() {
        let e = validate(16, 0).unwrap_err();
        assert!(e.contains("至少启用一类"), "零类错误文案要说人话: {e}");
        // 长度 < 类数：当前 MIN_LENGTH=8 > 4 类使其在公开入口不可达，
        // 但守卫必须真实存在（防未来下调下限/加类后保底被静默截断）
        let e = validate(3, 4).unwrap_err();
        assert!(e.contains("无法保证每类至少出现一个"), "{e}");
    }

    #[test]
    fn generate_rejects_bad_params_end_to_end() {
        let mut rng = splitmix_rng(7);
        assert!(generate_with(opts(7, true, true, true, true), &mut rng).is_err());
        assert!(generate_with(opts(16, false, false, false, false), &mut rng).is_err());
        assert!(generate_with(opts(16, true, false, false, false), &mut rng).is_ok());
    }

    #[test]
    fn generated_length_is_exact_across_range() {
        for len in [MIN_LENGTH, 9, DEFAULT_LENGTH, 33, MAX_LENGTH] {
            let mut rng = splitmix_rng(len as u64);
            let pw = generate_with(opts(len, true, true, true, true), &mut rng).unwrap();
            assert_eq!(pw.chars().count(), len, "长度必须逐字符精确: {pw}");
        }
    }

    #[test]
    fn every_enabled_class_appears_at_least_once() {
        // 全部 15 个非空类组合 × 代表长度 × 多样本：保底是构造性保证，
        // 任何一次缺类都是实现错误（不是概率问题）
        let combos = [
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, false, false, true),
            (true, true, false, false),
            (true, false, true, false),
            (true, false, false, true),
            (false, true, true, false),
            (false, true, false, true),
            (false, false, true, true),
            (true, true, true, false),
            (true, true, false, true),
            (true, false, true, true),
            (false, true, true, true),
            (true, true, true, true),
        ];
        let classes = [
            (b'A'..=b'Z').collect::<Vec<u8>>(),
            (b'a'..=b'z').collect::<Vec<u8>>(),
            (b'0'..=b'9').collect::<Vec<u8>>(),
            SYMBOLS.to_vec(),
        ];
        for (ci, &(u, l, d, s)) in combos.iter().enumerate() {
            let on = [u, l, d, s];
            for len in [MIN_LENGTH, DEFAULT_LENGTH, MAX_LENGTH] {
                for sample in 0..20u8 {
                    let mut rng =
                        splitmix_rng((sample as u64).wrapping_mul(37).wrapping_add(ci as u64 + 1));
                    let pw = generate_with(opts(len, u, l, d, s), &mut rng).unwrap();
                    let bytes = pw.as_bytes();
                    for (k, cs) in classes.iter().enumerate() {
                        if on[k] {
                            assert!(
                                bytes.iter().any(|b| cs.contains(b)),
                                "组合{ci} 长度{len} 样本{sample}: 启用类 {k} 未出现: {pw}"
                            );
                        } else {
                            assert!(
                                !bytes.iter().any(|b| cs.contains(b)),
                                "组合{ci}: 未启用的类 {k} 混进了字符: {pw}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_char_belongs_to_enabled_alphabet() {
        let mut rng = splitmix_rng(99);
        for &(u, l, d, s) in [
            (true, true, true, true),
            (false, true, true, false),
            (true, false, false, true),
            (false, false, true, true),
        ]
        .iter()
        {
            let pw = generate_with(opts(32, u, l, d, s), &mut rng).unwrap();
            for ch in pw.bytes() {
                let ok = (u && UPPER.contains(&ch))
                    || (l && LOWER.contains(&ch))
                    || (d && DIGITS.contains(&ch))
                    || (s && SYMBOLS.contains(&ch));
                assert!(ok, "字符 {} 不在任何启用类里: {pw}", ch as char);
            }
        }
    }

    #[test]
    fn exclude_ambiguous_removes_all_ambiguous_and_shrinks_alphabet() {
        let mut rng = splitmix_rng(11);
        let o = GenOptions {
            exclude_ambiguous: true,
            ..GenOptions::default()
        };
        for _ in 0..50 {
            let pw = generate_with(o, &mut rng).unwrap();
            for ch in pw.bytes() {
                assert!(
                    !AMBIGUOUS.contains(&ch),
                    "排除易混淆生效时出现 {} : {pw}",
                    ch as char
                );
            }
        }
        // 每类剔除后的字母表大小：24/25/8/28；全开 85；不排除 94
        let full = GenOptions::default();
        assert_eq!(
            enabled_classes(&full).iter().map(Vec::len).sum::<usize>(),
            94
        );
        let excluded = GenOptions {
            exclude_ambiguous: true,
            ..GenOptions::default()
        };
        let classes = enabled_classes(&excluded);
        let sizes: Vec<usize> = classes.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![24, 25, 8, 28], "剔除后各类大小必须与设计一致");
        assert_eq!(sizes.iter().sum::<usize>(), 85);
        // 只开符号：32 → 排除 | ' " ` 后 28
        let sym = opts(16, false, false, false, true);
        assert_eq!(enabled_classes(&sym)[0].len(), 32);
        let sym_ex = GenOptions {
            exclude_ambiguous: true,
            ..sym
        };
        assert_eq!(enabled_classes(&sym_ex)[0].len(), 28);
    }

    #[test]
    fn pick_index_is_exactly_uniform_rejection_included() {
        // 确定性证据：喂入 0..=199 各一次（n=100 → bound=200 全部接受），
        // 每个索引恰好命中 2 次——多一次少一次都是偏差实锤
        let data: Vec<u8> = (0u8..=199).collect();
        let idx = std::cell::Cell::new(0usize);
        let mut rng = |n: usize| {
            let start = idx.get();
            let end = (start + n).min(data.len());
            idx.set(end);
            data[start..end].to_vec()
        };
        let mut src = RngBytes {
            rng: &mut rng,
            buf: Vec::new(),
            pos: 0,
        };
        let mut counts = [0usize; 100];
        for _ in 0..200 {
            counts[pick_index(100, &mut src)] += 1;
        }
        assert!(
            counts.iter().all(|&c| c == 2),
            "均匀分布被破坏: {:?}",
            counts.iter().filter(|&&c| c != 2).collect::<Vec<_>>()
        );
        // 拒绝段：200/201 都 >= bound 必须被丢弃，首个接受字节是 5
        let rej: Vec<u8> = vec![200, 201, 5];
        let pos = std::cell::Cell::new(0usize);
        let mut rng2 = |n: usize| {
            let start = pos.get();
            let end = (start + n).min(rej.len());
            pos.set(end);
            rej[start..end].to_vec()
        };
        let mut src2 = RngBytes {
            rng: &mut rng2,
            buf: Vec::new(),
            pos: 0,
        };
        assert_eq!(pick_index(100, &mut src2), 5, "越界字节必须被拒绝采样丢弃");
    }

    #[test]
    fn injected_rng_is_deterministic_same_seed_same_password() {
        let a = generate_with(GenOptions::default(), &mut splitmix_rng(42)).unwrap();
        let b = generate_with(GenOptions::default(), &mut splitmix_rng(42)).unwrap();
        assert_eq!(a, b, "同种子必须产出同密码（纯函数核心可复现）");
        let c = generate_with(GenOptions::default(), &mut splitmix_rng(43)).unwrap();
        assert_ne!(a, c, "不同种子产出相同的概率必须极低");
    }

    #[test]
    fn real_csprng_entry_produces_valid_password() {
        // 生产入口：w32::random_bytes（BCryptGenRandom）。与 vault_crypto 的
        // random_key 测试同口径——系统 CSPRNG 在单测中可用且无副作用
        let pw = generate(GenOptions::default()).unwrap();
        assert_eq!(pw.chars().count(), DEFAULT_LENGTH);
        assert!(
            pw.bytes().all(|b| UPPER.contains(&b)
                || LOWER.contains(&b)
                || DIGITS.contains(&b)
                || SYMBOLS.contains(&b)),
            "生产输出必须落在四类字母表内: {pw}"
        );
    }
}
