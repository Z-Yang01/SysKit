//! 命令面板的纯逻辑：算式直算、模糊匹配、动作排序、自定义命令（对应 Python 侧
//! `palette_actions.py`）。
//!
//! ## 为什么算式求值要自己写解析器
//!
//! Python 用 `ast.parse` + 白名单算子（绝不用 `eval`）。Rust 没有等价的"安全子集
//! 求值"，所以这里写一个**只认数字与四则/幂/取模/括号**的递归下降解析器——
//! 词法上就不存在标识符、属性访问、函数调用，从根上不可能执行任意代码。
//!
//! ## 两个"不设就会冻结界面"的护栏（Python 版真机实测得来）
//!
//! 面板是在 GUI 线程同步求值的，一条小时级的大幂就是整个界面卡死：
//! 1. **指数绝对值 > 1000 直接拒**；
//! 2. 仅挡字面量指数还不够：`((10**1000)**1000)**1000` 的右操作数全是 1000，
//!    结果却是 330 万 bit。所以还要按**左操作数的 bit_length × 指数**估算结果规模，
//!    超界即拒——这一步必须在乘方**计算之前**做。

use serde::{Deserialize, Serialize};

/// 结果规模上限（bit）：约 200k bit 已经远超任何真实用途。
pub const MAX_RESULT_BITS: u64 = 200_000;
/// 指数绝对值上限。
pub const MAX_EXPONENT: i64 = 1000;

/// 一个面板动作。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaletteAction {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 动作来源分组（内置 / 自定义 / 剪贴板 / 密码库…），用于界面分区
    #[serde(default)]
    pub source: String,
}

/// 自定义命令（`settings.palette_commands`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCommand {
    pub name: String,
    /// `url`（浏览器打开）或 `cmd`（确认后执行）
    #[serde(rename = "type", default = "default_cmd_type")]
    pub kind: String,
    pub value: String,
}

fn default_cmd_type() -> String {
    "url".to_string()
}

// ---------------- 算式直算 ----------------

/// 输入是否"像算式"：仅由 ASCII 数字与运算符/括号/空白构成，且至少含一个运算符
/// 和一个数字。
///
/// **必须用 ASCII 数字判断**：Python 用 `[0-9]` 而不是 `\d`，因为 `\d` 会放行
/// 阿拉伯-印度数字等，导致"看起来像算式"与"求值器接受"两处口径不一致。
pub fn looks_like_math(q: &str) -> bool {
    let t = q.trim();
    if t.is_empty() {
        return false;
    }
    let only_math_chars = t
        .chars()
        .all(|c| c.is_ascii_digit() || " \t+-*/()%.".contains(c));
    if !only_math_chars {
        return false;
    }
    let has_op = t.chars().any(|c| "+-*/%".contains(c));
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    has_op && has_digit
}

/// 求值结果：整数或浮点。
///
/// 整数用 `i128` 而不是 `i64`：Python 的 int 是任意精度，`2**100` 这类常见
/// 算式在 Python 版能算出结果，`i64` 会在 2**63 处溢出。`i128` 覆盖到约
/// 1.7e38，能容纳实际会输入的规模；再大的由规模护栏拒绝（见 [`power`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcValue {
    Int(i128),
    Float(f64),
}

impl CalcValue {
    /// 展示文本（整值不带小数点，与 Python 一致）。
    pub fn display(&self) -> String {
        match self {
            CalcValue::Int(i) => i.to_string(),
            CalcValue::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() {
                    format!("{}", *f as i64)
                } else {
                    format!("{f}")
                }
            }
        }
    }
}

/// 安全求值：只支持数字、`+ - * / // % **`、一元正负、括号。
pub fn safe_calc(expr: &str) -> Result<CalcValue, String> {
    let tokens = tokenize(expr)?;
    let mut p = Parser { tokens, pos: 0 };
    let v = p.parse_expr()?;
    if p.pos != p.tokens.len() {
        return Err("表达式有多余内容".to_string());
    }
    // 溢出检查（与 Python 的 isinf/isnan 检查对应）
    if let CalcValue::Float(f) = v {
        if !f.is_finite() {
            return Err("结果溢出".to_string());
        }
    }
    Ok(v)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(CalcValue),
    Plus,
    Minus,
    Star,
    Slash,
    SlashSlash,
    Percent,
    StarStar,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let b: Vec<char> = s.trim().chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '*' => {
                if b.get(i + 1) == Some(&'*') {
                    out.push(Tok::StarStar);
                    i += 2;
                } else {
                    out.push(Tok::Star);
                    i += 1;
                }
            }
            '/' => {
                if b.get(i + 1) == Some(&'/') {
                    out.push(Tok::SlashSlash);
                    i += 2;
                } else {
                    out.push(Tok::Slash);
                    i += 1;
                }
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                let mut seen_dot = false;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                    if b[i] == '.' {
                        if seen_dot {
                            return Err("数字里出现多个小数点".to_string());
                        }
                        seen_dot = true;
                    }
                    i += 1;
                }
                // `2**3` 这类里 `2.` 也允许；`1.2.3` 已在上面拒绝
                let text: String = b[start..i].iter().collect();
                let text = text.trim_end_matches('.');
                if text.is_empty() {
                    return Err("数字格式非法".to_string());
                }
                if seen_dot {
                    let f: f64 = text.parse().map_err(|_| "数字格式非法".to_string())?;
                    out.push(Tok::Num(CalcValue::Float(f)));
                } else {
                    match text.parse::<i128>() {
                        Ok(n) => out.push(Tok::Num(CalcValue::Int(n))),
                        // 超出 i64 的字面量按浮点处理（Python 是大整数，这里退一步）
                        Err(_) => {
                            let f: f64 = text.parse().map_err(|_| "数字格式非法".to_string())?;
                            out.push(Tok::Num(CalcValue::Float(f)));
                        }
                    }
                }
            }
            other => return Err(format!("不支持的字符: {other}")),
        }
    }
    if out.is_empty() {
        return Err("空表达式".to_string());
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn eat(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// expr := term (('+'|'-') term)*
    fn parse_expr(&mut self) -> Result<CalcValue, String> {
        let mut left = self.parse_term()?;
        while let Some(op) = self.peek() {
            let op = op.clone();
            match op {
                Tok::Plus | Tok::Minus => {
                    self.pos += 1;
                    let right = self.parse_term()?;
                    left = if matches!(op, Tok::Plus) {
                        add(left, right)?
                    } else {
                        sub(left, right)?
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// term := power (('*'|'/'|'//'|'%') power)*
    fn parse_term(&mut self) -> Result<CalcValue, String> {
        let mut left = self.parse_power()?;
        while let Some(op) = self.peek() {
            let op = op.clone();
            match op {
                Tok::Star | Tok::Slash | Tok::SlashSlash | Tok::Percent => {
                    self.pos += 1;
                    let right = self.parse_power()?;
                    left = match op {
                        Tok::Star => mul(left, right)?,
                        Tok::Slash => div(left, right)?,
                        Tok::SlashSlash => floordiv(left, right)?,
                        _ => modulo(left, right)?,
                    };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// power := unary ('**' power)?   —— 右结合
    fn parse_power(&mut self) -> Result<CalcValue, String> {
        let base = self.parse_unary()?;
        if matches!(self.peek(), Some(Tok::StarStar)) {
            self.pos += 1;
            let exp = self.parse_power()?;
            return power(base, exp);
        }
        Ok(base)
    }

    /// unary := ('+'|'-')* primary
    fn parse_unary(&mut self) -> Result<CalcValue, String> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(neg(v))
            }
            Some(Tok::Plus) => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<CalcValue, String> {
        match self.eat() {
            Some(Tok::Num(v)) => Ok(v),
            Some(Tok::LParen) => {
                let v = self.parse_expr()?;
                match self.eat() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err("括号不匹配".to_string()),
                }
            }
            Some(t) => Err(format!("意外的记号: {t:?}")),
            None => Err("表达式不完整".to_string()),
        }
    }
}

fn as_f64(v: CalcValue) -> f64 {
    match v {
        CalcValue::Int(i) => i as f64,
        CalcValue::Float(f) => f,
    }
}

fn add(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    match (a, b) {
        (CalcValue::Int(x), CalcValue::Int(y)) => x
            .checked_add(y)
            .map(CalcValue::Int)
            .ok_or_else(|| "整数溢出（超出 i128 范围）".to_string()),
        _ => Ok(CalcValue::Float(as_f64(a) + as_f64(b))),
    }
}

fn sub(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    match (a, b) {
        (CalcValue::Int(x), CalcValue::Int(y)) => x
            .checked_sub(y)
            .map(CalcValue::Int)
            .ok_or_else(|| "整数溢出（超出 i128 范围）".to_string()),
        _ => Ok(CalcValue::Float(as_f64(a) - as_f64(b))),
    }
}

fn mul(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    match (a, b) {
        (CalcValue::Int(x), CalcValue::Int(y)) => x
            .checked_mul(y)
            .map(CalcValue::Int)
            .ok_or_else(|| "整数溢出（超出 i128 范围）".to_string()),
        _ => Ok(CalcValue::Float(as_f64(a) * as_f64(b))),
    }
}

fn div(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    let (x, y) = (as_f64(a), as_f64(b));
    if y == 0.0 {
        return Err("除数为 0".to_string());
    }
    Ok(CalcValue::Float(x / y))
}

fn floordiv(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    match (a, b) {
        (CalcValue::Int(x), CalcValue::Int(y)) => {
            if y == 0 {
                return Err("除数为 0".to_string());
            }
            Ok(CalcValue::Int(x.div_euclid(y)))
        }
        _ => {
            let (x, y) = (as_f64(a), as_f64(b));
            if y == 0.0 {
                return Err("除数为 0".to_string());
            }
            Ok(CalcValue::Float((x / y).floor()))
        }
    }
}

fn modulo(a: CalcValue, b: CalcValue) -> Result<CalcValue, String> {
    match (a, b) {
        (CalcValue::Int(x), CalcValue::Int(y)) => {
            if y == 0 {
                return Err("除数为 0".to_string());
            }
            Ok(CalcValue::Int(x.rem_euclid(y)))
        }
        _ => {
            let (x, y) = (as_f64(a), as_f64(b));
            if y == 0.0 {
                return Err("除数为 0".to_string());
            }
            Ok(CalcValue::Float(x % y))
        }
    }
}

fn neg(v: CalcValue) -> CalcValue {
    match v {
        CalcValue::Int(i) => CalcValue::Int(-i),
        CalcValue::Float(f) => CalcValue::Float(-f),
    }
}

/// 幂运算，带两道**计算前**的规模护栏（见模块文档）。
fn power(base: CalcValue, exp: CalcValue) -> Result<CalcValue, String> {
    let e = match exp {
        CalcValue::Int(i) => i,
        CalcValue::Float(f) => {
            if f.fract() != 0.0 {
                return Err("指数必须是整数".to_string());
            }
            f as i128
        }
    };
    if e.abs() > MAX_EXPONENT as i128 {
        return Err("指数过大".to_string());
    }
    match base {
        CalcValue::Int(b) => {
            if e < 0 {
                // 负指数：整数底数会得到小数，按浮点算
                let r = (b as f64).powi(e as i32);
                if !r.is_finite() {
                    return Err("结果溢出".to_string());
                }
                return Ok(CalcValue::Float(r));
            }
            if e == 0 {
                return Ok(CalcValue::Int(1));
            }
            // 护栏一（比例式）：**在计算前**估算结果规模。
            // 只挡字面量指数挡不住 ((10**1000)**1000)**1000——右操作数全是 1000，
            // 结果却是 330 万 bit，面板在 GUI 线程同步求值 → 整个界面冻结。
            let bits = (128 - b.unsigned_abs().leading_zeros()) as u64 * e as u64;
            if bits > MAX_RESULT_BITS {
                return Err("结果过大".to_string());
            }
            // 护栏二：精确的溢出检测交给 checked_pow。
            // 不要用位长公式预判——`ilog2` 会把 10 当成 4 bit（实际 3.32），
            // 于是 `10**38`（实际 127 bit，装得下）被误拒。checked_pow 是精确的。
            match b.checked_pow(e as u32) {
                Some(v) => Ok(CalcValue::Int(v)),
                None => Err("结果过大（超出整数范围，请用小一点的指数）".to_string()),
            }
        }
        CalcValue::Float(b) => {
            let r = b.powi(e as i32);
            if !r.is_finite() {
                return Err("结果溢出".to_string());
            }
            Ok(CalcValue::Float(r))
        }
    }
}

// ---------------- 模糊匹配 ----------------

/// 子序列匹配得分；不匹配返回 None。连续命中/前缀/词首加分，短文本微加分。
///
/// 与 Python 逐行同口径（含"命中位置 0 加 2 分""前一字符是分隔符加 1 分"
/// "末尾按长度微减分"这些细节）——两边分数不同会让同一个查询在两版里
/// 排出不同顺序，用户会觉得"搜索结果变了"。
pub fn fuzzy_score(query: &str, text: &str) -> Option<f64> {
    let q = query.to_lowercase().replace(' ', "");
    let t = text.to_lowercase();
    if q.is_empty() {
        return Some(0.0);
    }
    if t.is_empty() {
        return None;
    }
    let tc: Vec<char> = t.chars().collect();
    let mut score = 0.0f64;
    let mut ti = 0usize;
    let mut streak = 0i64;
    for ch in q.chars() {
        let idx = tc[ti..].iter().position(|&c| c == ch)? + ti;
        streak = if idx == ti { streak + 1 } else { 1 };
        score += 1.0 + streak as f64 * 0.5;
        if idx == 0 {
            score += 2.0;
        } else if ti == 0 || " -_./".contains(tc[ti - 1]) {
            score += 1.0;
        }
        ti = idx + 1;
    }
    Some(score - tc.len() as f64 * 0.01)
}

/// label 为主、keywords 为辅（半权）。
pub fn match_action(query: &str, action: &PaletteAction) -> Option<f64> {
    let mut s = fuzzy_score(query, &action.label);
    let kw = action.keywords.join(" ");
    if !kw.is_empty() {
        if let Some(s2) = fuzzy_score(query, &kw) {
            s = Some(s.map_or(s2 * 0.5, |v| v.max(s2 * 0.5)));
        }
    }
    s
}

/// 按得分降序返回可行动作；query 为空时原样返回全部。
///
/// 排序稳定性很重要：得分相同的动作必须保持注册顺序，否则同一个查询每次
/// 打开面板的顺序都可能不同。
pub fn rank_actions(query: &str, actions: &[PaletteAction]) -> Vec<PaletteAction> {
    if query.trim().is_empty() {
        return actions.to_vec();
    }
    let mut scored: Vec<(f64, usize, PaletteAction)> = actions
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match_action(query, a).map(|s| (s, i, a.clone())))
        .collect();
    // 得分降序；同分按原下标升序（稳定）
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored.into_iter().map(|(_, _, a)| a).collect()
}

/// 解析 `settings.palette_commands`（JSON 字符串）→ 自定义命令。
///
/// 解析失败返回空表：面板是常用入口，一条坏配置不该让它整体不可用。
/// 缺 `name`/`value` 的条目直接丢弃（没有名字就没法展示、没有值就无法执行）。
pub fn parse_custom_commands(raw: &str) -> Vec<CustomCommand> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|it| {
            let obj = it.as_object()?;
            let name = obj.get("name")?.as_str()?.trim();
            let value = obj.get("value")?.as_str()?.trim();
            if name.is_empty() || value.is_empty() {
                return None;
            }
            let kind = obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("url")
                .to_string();
            // 未知类型按 url 处理（保守：url 需要显式点击才打开，cmd 会执行命令）
            let kind = if kind == "cmd" { "cmd" } else { "url" }.to_string();
            Some(CustomCommand {
                name: name.to_string(),
                kind,
                value: value.to_string(),
            })
        })
        .collect()
}

/// 内置动作注册表（对齐 Python `palette_actions.build_actions` 的条目集）。
///
/// Rust 面板没有窗口回调——动作分两类由前端执行：
/// - `nav:<key>` 页面跳转（前端 setPage）
/// - `quick:*` 前端已有能力的行为（跳页 + 触发既有按钮），或后端命令
/// 自定义命令照旧走 custom:*。
pub fn builtin_actions() -> Vec<PaletteAction> {
    let nav_pages: [(&str, &str); 7] = [
        ("overview", "🏠 概览 / 网络监控"),
        ("clipboard", "📋 剪贴板历史"),
        ("diagnose", "🔍 诊断 / 修复"),
        ("toolbox", "⚡ 工具箱"),
        ("system", "💼 系统助手"),
        ("vault", "🔒 密码库"),
        ("settings", "⚙ 设置 / 日志"),
    ];
    let mut out: Vec<PaletteAction> = nav_pages
        .iter()
        .map(|(key, label)| PaletteAction {
            id: format!("nav:{key}"),
            label: format!("打开 {label}"),
            hint: "页面".into(),
            keywords: vec!["goto".into(), "页面".into(), "打开".into()],
            source: "页面".into(),
        })
        .collect();
    // 页内 tab（对齐 Python 的页内 Tab 动作；前端映射到子 tab key）
    let tabs: [(&str, &str, &str, &str); 11] = [
        ("toolbox", "hosts", "⚡ 工具箱", "Hosts 编辑"),
        ("toolbox", "ports", "⚡ 工具箱", "端口 / 局域网"),
        ("toolbox", "dns", "⚡ 工具箱", "DNS / 子网 / ping"),
        ("toolbox", "speed", "⚡ 工具箱", "测速"),
        ("toolbox", "appnet", "⚡ 工具箱", "应用联网"),
        ("system", "perf", "💼 系统助手", "性能监控"),
        ("system", "startup", "💼 系统助手", "启动项管理"),
        ("system", "disk", "💼 系统助手", "磁盘空间"),
        ("system", "clean", "💼 系统助手", "垃圾清理"),
        ("system", "battery", "💼 系统助手", "电池健康"),
        ("system", "modes", "💼 系统助手", "场景模式"),
    ];
    for (page, tab, page_label, tab_label) in tabs {
        out.push(PaletteAction {
            id: format!("tab:{page}:{tab}"),
            label: format!("{page_label} · {tab_label}"),
            hint: "工具".into(),
            keywords: vec![tab_label.to_string()],
            source: "页面".into(),
        });
    }
    // 快捷动作（对齐 Python quick:*；wifi 对齐 WiFi 工具页入口）
    for (id, label, hint, kw) in [
        ("quick:diag", "立即快速诊断", "网络", vec!["diagnose", "体检", "检测"]),
        ("quick:deep", "深度诊断（全部检查）", "网络", vec!["diagnose", "完整"]),
        ("quick:flush", "刷新 DNS 缓存", "网络", vec!["flush", "修复", "dns"]),
        ("quick:wifi", "打开 WiFi 工具", "网络", vec!["wifi", "无线", "AP"]),
        ("quick:show", "显示主界面", "窗口", vec!["show", "主窗口"]),
        (
            "quick:clip_paste",
            "剪贴板快捷粘贴面板",
            "剪贴板",
            vec!["clipboard", "粘贴", "paste", "历史"],
        ),
    ] {
        out.push(PaletteAction {
            id: id.into(),
            label: label.into(),
            hint: hint.into(),
            keywords: kw.into_iter().map(Into::into).collect(),
            source: "快捷".into(),
        });
    }
    out
}

/// 一次面板查询：算式直算 / 内置+自定义动作模糊排序（对齐 Python
/// `_dynamic_actions` 的两段式：算式命中直接给结果项，否则按匹配分排序动作）。
pub fn query(query: &str, custom_raw: &str) -> Vec<(String, String, String)> {
    let q = query.trim();
    let mut out: Vec<(String, String, String)> = Vec::new();
    if looks_like_math(q) {
        if let Ok(v) = safe_calc(q) {
            out.push((
                "calc".into(),
                format!("{q} = {}", v.display()),
                "算式（回车复制结果）".into(),
            ));
        }
    }
    let mut actions = builtin_actions();
    actions.extend(custom_to_actions(&parse_custom_commands(custom_raw)));
    for a in rank_actions(q, &actions) {
        out.push((a.id, a.label, a.hint));
    }
    out
}

/// 自定义命令 → 可参与匹配与执行的动作。
pub fn custom_to_actions(cmds: &[CustomCommand]) -> Vec<PaletteAction> {
    cmds.iter()
        .enumerate()
        .map(|(i, c)| PaletteAction {
            id: format!("custom:{i}"),
            label: c.name.clone(),
            hint: if c.kind == "cmd" {
                "自定义命令（会先确认）".to_string()
            } else {
                "打开网址".to_string()
            },
            keywords: vec!["自定义".to_string(), c.kind.clone()],
            source: "自定义".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(id: &str, label: &str, kw: &[&str]) -> PaletteAction {
        PaletteAction {
            id: id.into(),
            label: label.into(),
            hint: String::new(),
            keywords: kw.iter().map(|s| s.to_string()).collect(),
            source: String::new(),
        }
    }

    // ---------------- looks_like_math ----------------

    #[test]
    fn looks_like_math_accepts_plain_arithmetic() {
        for q in [
            "1+1", "2*3", "10/4", "1+2*3", "(1+2)*3", "5%2", "2**10", "1.5*2", " 1 + 1 ",
        ] {
            assert!(looks_like_math(q), "{q} 应被认成算式");
        }
    }

    #[test]
    fn looks_like_math_rejects_non_math() {
        for q in ["", "abc", "打开诊断", "1+1 请", "http://x.com", "0x10"] {
            assert!(!looks_like_math(q), "{q} 不该被认成算式");
        }
        // 注意：`1+` **会**被判为"像算式"——Python 版同样如此（它只看字符构成，
        // 不校验语法）。真正的拒绝发生在 safe_calc 里。这里锁住这个分工，
        // 免得后人"顺手加严"而与 Python 版口径分叉。
        assert!(looks_like_math("1+"), "形状判定不负责语法校验");
        assert!(safe_calc("1+").is_err(), "语法错误由求值器负责拒绝");
    }

    #[test]
    fn looks_like_math_requires_both_operator_and_digit() {
        assert!(!looks_like_math("123"), "没有运算符");
        assert!(!looks_like_math("+*()"), "没有数字");
    }

    #[test]
    fn looks_like_math_rejects_non_ascii_digits() {
        // Python 注释专门写了这条：\d 会放行阿拉伯-印度数字，而求值器只认 ASCII
        assert!(!looks_like_math("١+١"), "非 ASCII 数字不该被认成算式");
        assert!(!looks_like_math("１+１"), "全角数字同样拒绝");
    }

    // ---------------- safe_calc ----------------

    #[test]
    fn safe_calc_basic_arithmetic() {
        assert_eq!(safe_calc("1+1").unwrap(), CalcValue::Int(2));
        assert_eq!(safe_calc("7-10").unwrap(), CalcValue::Int(-3));
        assert_eq!(safe_calc("6*7").unwrap(), CalcValue::Int(42));
        assert_eq!(safe_calc("(1+2)*3").unwrap(), CalcValue::Int(9));
        assert_eq!(safe_calc("2**10").unwrap(), CalcValue::Int(1024));
        assert_eq!(safe_calc("7%3").unwrap(), CalcValue::Int(1));
        assert_eq!(safe_calc("7//2").unwrap(), CalcValue::Int(3));
        assert_eq!(safe_calc("7/2").unwrap(), CalcValue::Float(3.5));
    }

    #[test]
    fn safe_calc_power_is_right_associative() {
        // 2**3**2 = 2**(3**2) = 512，不是 (2**3)**2 = 64
        assert_eq!(safe_calc("2**3**2").unwrap(), CalcValue::Int(512));
    }

    #[test]
    fn safe_calc_unary_operators() {
        assert_eq!(safe_calc("-5+3").unwrap(), CalcValue::Int(-2));
        assert_eq!(safe_calc("+5").unwrap(), CalcValue::Int(5));
        assert_eq!(safe_calc("--5").unwrap(), CalcValue::Int(5));
        assert_eq!(safe_calc("-(1+2)").unwrap(), CalcValue::Int(-3));
    }

    #[test]
    fn safe_calc_division_by_zero_is_an_error() {
        assert!(safe_calc("1/0").is_err());
        assert!(safe_calc("1//0").is_err());
        assert!(safe_calc("1%0").is_err());
        assert!(safe_calc("1/0").unwrap_err().contains('0'));
    }

    #[test]
    fn safe_calc_rejects_huge_exponent() {
        // 护栏一：指数本身过大
        let e = safe_calc("10**100000").unwrap_err();
        assert!(e.contains("指数过大"), "{e}");
    }

    #[test]
    fn safe_calc_rejects_chained_huge_power() {
        // 护栏二（真机实测来的）：只挡字面量指数挡不住这个链。
        // ((10**1000)**1000)**1000 的右操作数全是 1000，结果却是 330 万 bit，
        // 面板在 GUI 线程同步求值 → 整个界面冻结。
        let e = safe_calc("((10**1000)**1000)**1000").unwrap_err();
        assert!(
            e.contains("过大") || e.contains("溢出"),
            "必须在计算前按规模拒绝，实际: {e}"
        );
    }

    #[test]
    fn safe_calc_allows_powers_within_i128() {
        // i128 上限约 1.7e38：这些常见算式必须能算出结果
        assert_eq!(safe_calc("2**10").unwrap(), CalcValue::Int(1024));
        assert_eq!(
            safe_calc("2**64").unwrap(),
            CalcValue::Int(18_446_744_073_709_551_616)
        );
        assert_eq!(
            safe_calc("2**100").unwrap(),
            CalcValue::Int(1_267_650_600_228_229_401_496_703_205_376)
        );
        // 10**38 仍在 i128 内
        assert!(safe_calc("10**38").is_ok());
    }

    #[test]
    fn safe_calc_rejects_powers_beyond_i128_with_a_clear_reason() {
        // 10**39 超出 i128（Python 的 int 是任意精度，Rust 有硬上限）。
        // 必须**明确报"超出整数范围"**而不是静默溢出成错误数字——
        // 静默给出错误结果比报错危险得多（用户会拿它算东西）。
        let e = safe_calc("10**40").unwrap_err();
        assert!(e.contains("超出整数范围"), "{e}");
        // 10**100 同理（Python 能算，这里诚实地报不行）
        assert!(safe_calc("10**100").is_err());
    }

    #[test]
    fn safe_calc_rejects_code_like_input() {
        // 词法上就不存在标识符/调用，从根上不可能执行任意代码
        for bad in ["__import__('os')", "print(1)", "a+1", "1;2", "open('x')"] {
            assert!(safe_calc(bad).is_err(), "{bad} 必须被拒绝");
        }
    }

    #[test]
    fn safe_calc_rejects_malformed() {
        assert!(safe_calc("").is_err());
        assert!(safe_calc("(1+2").is_err(), "括号不匹配");
        assert!(safe_calc("1+2)").is_err());
        assert!(safe_calc("1+").is_err(), "表达式不完整");
        assert!(safe_calc("1 2").is_err(), "多余内容");
    }

    #[test]
    fn query_returns_calc_first_and_ranks_actions() {
        // 算式命中：直接给结果项（对齐 Python _dynamic_actions 第一段）
        let r = query("1+2*3", "");
        assert!(r.iter().any(|(id, label, _)| id == "calc" && label.contains("7")));
        // 空查询：返回全部内置动作（页面 + 页内 tab + 快捷）
        let r = query("", "");
        assert!(r.iter().any(|(_, label, _)| label.contains("概览")));
        assert!(r.iter().any(|(id, _, _)| id == "tab:toolbox:speed"));
        assert!(r.iter().any(|(id, _, _)| id == "quick:flush"));
        // 关键词过滤：搜 "dns" 至少命中 Hosts 之外的 DNS 相关 tab 与 quick:flush
        let r = query("dns", "");
        assert!(r.iter().any(|(id, _, _)| id == "quick:flush"));
        assert!(r.iter().any(|(_, label, _)| label.contains("DNS")));
        // 自定义命令参与匹配
        let r = query("github", r#"[{"name":"打开 GitHub","type":"url","value":"https://github.com"}]"#);
        assert!(r.iter().any(|(id, label, _)| id.starts_with("custom:") && label.contains("GitHub")));
    }

    #[test]
    fn calc_value_display_drops_trailing_zero() {
        assert_eq!(CalcValue::Int(42).display(), "42");
        assert_eq!(CalcValue::Float(4.0).display(), "4");
        assert_eq!(CalcValue::Float(3.5).display(), "3.5");
    }

    #[test]
    fn safe_calc_float_results() {
        assert_eq!(safe_calc("1.5*2").unwrap(), CalcValue::Float(3.0));
        assert_eq!(
            safe_calc("0.1+0.2").unwrap().display(),
            "0.30000000000000004"
        );
    }

    // ---------------- 模糊匹配 ----------------

    #[test]
    fn fuzzy_score_matches_subsequences() {
        // 子序列匹配：查询字符按序出现即可，不必连续（用拉丁字母演示，
        // 中文标签里查拉丁串本来就匹配不上——下一条用例锁住这点）
        assert!(fuzzy_score("gt", "goto page").is_some());
        assert!(fuzzy_score("dp", "打开 diagnostic page").is_some());
        assert!(fuzzy_score("打开", "打开诊断").is_some(), "中文连续命中");
        assert_eq!(fuzzy_score("xyz", "打开诊断"), None, "不匹配返回 None");
    }

    #[test]
    fn fuzzy_score_is_literal_not_transliterated() {
        // 查拉丁串不会命中纯中文标签（Python 同样返回 None）：
        // 这决定了中文界面下得靠中文或拼音关键词，而不是"能猜出拼音"
        assert_eq!(fuzzy_score("dr", "打开诊断"), None);
        assert_eq!(fuzzy_score("dkzd", "打开诊断"), None);
    }

    #[test]
    fn fuzzy_score_empty_query_is_zero() {
        assert_eq!(fuzzy_score("", "anything"), Some(0.0));
        assert_eq!(
            fuzzy_score("   ", "anything"),
            Some(0.0),
            "空格被剔除后为空"
        );
    }

    #[test]
    fn fuzzy_score_empty_text_is_none() {
        assert_eq!(fuzzy_score("a", ""), None);
    }

    #[test]
    fn fuzzy_score_prefers_prefix_and_contiguous() {
        // 前缀命中应高于中间命中
        let prefix = fuzzy_score("abc", "abcdef").unwrap();
        let middle = fuzzy_score("abc", "xxabcxx").unwrap();
        assert!(prefix > middle, "前缀 {prefix} 应高于中间 {middle}");
    }

    #[test]
    fn fuzzy_score_shorter_text_wins_on_same_query() {
        let short = fuzzy_score("ab", "ab").unwrap();
        let long = fuzzy_score("ab", "abxxxxxxxxxxxxxxxx").unwrap();
        assert!(short > long, "短文本微加分: {short} vs {long}");
    }

    #[test]
    fn fuzzy_score_ignores_spaces_in_query() {
        assert_eq!(fuzzy_score("a b", "ab"), fuzzy_score("ab", "ab"));
    }

    #[test]
    fn match_action_keywords_count_half() {
        let a = act("1", "完全不相干的名字", &["诊断"]);
        let via_label = match_action("完全", &a).unwrap();
        let via_kw = match_action("诊断", &a).unwrap();
        assert!(via_label > 0.0);
        assert!(via_kw > 0.0, "关键词也能命中");
        // 同样命中时 label 的分数应高于 keywords（半权）
        let b = act("2", "诊断", &[]);
        assert!(match_action("诊断", &b).unwrap() > via_kw);
    }

    #[test]
    fn rank_actions_empty_query_keeps_order() {
        let actions = vec![act("a", "A", &[]), act("b", "B", &[])];
        let r = rank_actions("", &actions);
        assert_eq!(
            r.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn rank_actions_sorts_by_score_desc() {
        let actions = vec![
            act("mid", "xx诊断yy", &[]),
            act("best", "诊断", &[]),
            act("none", "完全无关", &[]),
        ];
        let r = rank_actions("诊断", &actions);
        assert_eq!(r[0].id, "best");
        assert!(r.iter().all(|a| a.id != "none"), "不匹配的应被过滤");
    }

    #[test]
    fn rank_actions_ties_keep_registration_order() {
        // 同分必须稳定：否则同一个查询每次打开面板顺序都可能不同
        let actions = vec![
            act("a", "same", &[]),
            act("b", "same", &[]),
            act("c", "same", &[]),
        ];
        let r = rank_actions("same", &actions);
        assert_eq!(
            r.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    // ---------------- 自定义命令 ----------------

    #[test]
    fn parse_custom_commands_valid() {
        let raw = r#"[{"name":"公司内网","type":"url","value":"https://x.com"},
                      {"name":"清理","type":"cmd","value":"cleanmgr"}]"#;
        let c = parse_custom_commands(raw);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "公司内网");
        assert_eq!(c[0].kind, "url");
        assert_eq!(c[1].kind, "cmd");
    }

    #[test]
    fn parse_custom_commands_tolerates_bad_input() {
        // 面板是常用入口，一条坏配置不该让它整体不可用
        assert!(parse_custom_commands("").is_empty());
        assert!(parse_custom_commands("不是 json").is_empty());
        assert!(parse_custom_commands("{}").is_empty(), "不是数组");
        assert!(parse_custom_commands("[1,2,3]").is_empty());
        assert!(
            parse_custom_commands(r#"[{"name":"x"}]"#).is_empty(),
            "缺 value"
        );
        assert!(
            parse_custom_commands(r#"[{"value":"x"}]"#).is_empty(),
            "缺 name"
        );
        assert!(
            parse_custom_commands(r#"[{"name":"  ","value":"x"}]"#).is_empty(),
            "空白名字无效"
        );
    }

    #[test]
    fn parse_custom_commands_defaults_type_to_url() {
        let c = parse_custom_commands(r#"[{"name":"a","value":"https://a"}]"#);
        assert_eq!(c[0].kind, "url", "缺 type 时默认 url（保守方向）");
    }

    #[test]
    fn parse_custom_commands_normalizes_unknown_type_to_url() {
        // 未知类型收敛成 url 而不是 cmd：url 需要显式点击才打开，
        // cmd 会执行命令——保守方向必须选前者
        let c = parse_custom_commands(r#"[{"name":"a","value":"x","type":"weird"}]"#);
        assert_eq!(c[0].kind, "url");
    }

    #[test]
    fn custom_to_actions_maps_fields() {
        let cmds = vec![
            CustomCommand {
                name: "内网".into(),
                kind: "url".into(),
                value: "https://x".into(),
            },
            CustomCommand {
                name: "清理".into(),
                kind: "cmd".into(),
                value: "cleanmgr".into(),
            },
        ];
        let a = custom_to_actions(&cmds);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].label, "内网");
        assert_eq!(a[0].source, "自定义");
        assert!(a[1].hint.contains("确认"), "cmd 类型要提示会先确认");
    }

    #[test]
    fn custom_actions_participate_in_ranking() {
        let cmds = parse_custom_commands(r#"[{"name":"打开公司内网","value":"https://x"}]"#);
        let actions = custom_to_actions(&cmds);
        let r = rank_actions("公司", &actions);
        assert_eq!(r.len(), 1, "自定义命令应能被搜到");
    }
}
