//! DPAPI 密封与 BCrypt（随机数 / PBKDF2 / AES-GCM）。
use super::*;

/// DPAPI 密封（绑定当前用户）。失败返回 None（UI 线程静默降级）。
pub fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // CryptProtectData 用 LocalAlloc 分配输出，必须由调用方 LocalFree
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(v)
    }
}

/// DPAPI 解密。失败（换用户/截断）返回 None。
pub fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // CryptUnprotectData 同样用 LocalAlloc 分配输出，必须由调用方 LocalFree
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(v)
    }
}

// ---------------- 随机数 / PBKDF2 / AES-GCM（BCrypt） ----------------

/// 生成 `n` 字节密码学随机数（`BCryptGenRandom`，系统 CSPRNG）。
///
/// 不用时间戳或自建 PRNG：数据密钥与 nonce 的随机性直接决定安全性。
pub fn random_bytes(n: usize) -> Vec<u8> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut buf = vec![0u8; n];
    // SAFETY: buf 是本地 Vec，长度与传入的 n 一致
    let st = unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if st.is_err() {
        // 取不到随机数绝不能"退化"成可预测的值——那会让密钥可被猜测
        panic!("BCryptGenRandom 失败: 无法获取密码学随机数");
    }
    buf
}

/// PBKDF2-HMAC-SHA256 派生密钥。
pub fn pbkdf2_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out_len: usize,
) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDeriveKeyPBKDF2, BCryptOpenAlgorithmProvider,
        BCRYPT_ALG_HANDLE, BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_SHA256_ALGORITHM,
    };

    let mut alg = BCRYPT_ALG_HANDLE::default();
    // SAFETY: 句柄成对释放；输入切片在调用期间有效
    unsafe {
        // PBKDF2 要求以 HMAC 方式打开 PRF 算法
        let st = BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            None,
            BCRYPT_ALG_HANDLE_HMAC_FLAG,
        );
        if st.is_err() {
            return Err("BCryptOpenAlgorithmProvider(SHA256/HMAC) 失败".to_string());
        }
        let mut out = vec![0u8; out_len];
        // 迭代数是 u64（BCrypt 的 ABI 如此），出参要传 &mut [u8] 而不是 Option
        let st = BCryptDeriveKeyPBKDF2(
            alg,
            Some(password),
            Some(salt),
            iterations as u64,
            &mut out,
            0,
        );
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        if st.is_err() {
            return Err("BCryptDeriveKeyPBKDF2 失败".to_string());
        }
        Ok(out)
    }
}

/// AES-256-GCM 加密，返回 `ct || tag`（不含 nonce——外层拼装由调用方负责，
/// 与 Python `AESGCM.encrypt` 的返回一致）。
pub fn aes_gcm_encrypt(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
    tag_len: usize,
) -> Result<Vec<u8>, String> {
    let mut tag = vec![0u8; tag_len];
    // GCM 的密文与明文等长
    let mut out = vec![0u8; plaintext.len()];
    let info = auth_info(nonce, aad, &mut tag);
    let done = bcrypt_gcm(key, &info, true, plaintext, &mut out)?;
    if done != plaintext.len() as u32 {
        return Err(format!("GCM 加密返回长度异常: {done}"));
    }
    out.extend_from_slice(&tag);
    Ok(out)
}

/// AES-256-GCM 解密，输入 `ct || tag`（不含 nonce）。失败即认证失败。
pub fn aes_gcm_decrypt(
    key: &[u8],
    nonce: &[u8],
    ct_and_tag: &[u8],
    aad: &[u8],
    tag_len: usize,
) -> Result<Vec<u8>, String> {
    if ct_and_tag.len() < tag_len {
        return Err("密文长度不足（缺少认证标签）".to_string());
    }
    let ct_len = ct_and_tag.len() - tag_len;
    let (ct, tag) = ct_and_tag.split_at(ct_len);
    let mut tag_buf = tag.to_vec();
    let info = auth_info(nonce, aad, &mut tag_buf);
    let mut out = vec![0u8; ct_len];
    let done = bcrypt_gcm(key, &info, false, ct, &mut out)?;
    if done != ct_len as u32 {
        return Err(format!("GCM 解密返回长度异常: {done}"));
    }
    Ok(out)
}

/// 构造 GCM 的认证模式信息（`cbSize` 与版本号必须正确填写，否则 BCrypt 拒绝）。
fn auth_info(
    nonce: &[u8],
    aad: &[u8],
    tag: &mut [u8],
) -> windows::Win32::Security::Cryptography::BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
    use windows::Win32::Security::Cryptography::{
        BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
    };
    BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
        cbSize: std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
        dwInfoVersion: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
        // 这些指针只在调用期间有效
        pbNonce: nonce.as_ptr() as *mut u8,
        cbNonce: nonce.len() as u32,
        pbAuthData: aad.as_ptr() as *mut u8,
        cbAuthData: aad.len() as u32,
        pbTag: tag.as_mut_ptr(),
        cbTag: tag.len() as u32,
        pbMacContext: std::ptr::null_mut(),
        cbMacContext: 0,
        cbAAD: 0,
        cbData: 0,
        dwFlags: 0,
    }
}

/// 打开 AES-GCM 算法 + 导入密钥，执行一次加密或解密，返回写入的字节数。
fn bcrypt_gcm(
    key: &[u8],
    info: &windows::Win32::Security::Cryptography::BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO,
    encrypt: bool,
    input: &[u8],
    out: &mut [u8],
) -> Result<u32, String> {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDecrypt, BCryptDestroyKey, BCryptEncrypt,
        BCryptGenerateSymmetricKey, BCryptOpenAlgorithmProvider, BCryptSetProperty,
        BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE, BCRYPT_CHAINING_MODE, BCRYPT_CHAIN_MODE_GCM,
        BCRYPT_FLAGS, BCRYPT_HANDLE, BCRYPT_KEY_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
    };

    let mut alg = BCRYPT_ALG_HANDLE::default();
    let mut kh = BCRYPT_KEY_HANDLE::default();
    // SAFETY: 句柄成对释放；info/input/out 的生命周期覆盖调用
    unsafe {
        if BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_AES_ALGORITHM,
            None,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .is_err()
        {
            return Err("BCryptOpenAlgorithmProvider(AES) 失败".to_string());
        }
        // **必须**显式设成 GCM：默认链模式是 CBC，不设就完全不是 GCM。
        // 这个属性的值是"宽字符串常量"而不是普通缓冲——**必须带结尾 NUL**，
        // BCrypt 会做 NUL 结尾的字符串比较（`ChainingModeGCM` 不带 NUL 会返回
        // STATUS_INVALID_PARAMETER，本机就是这样失败的）。
        let mode_str: Vec<u16> = BCRYPT_CHAIN_MODE_GCM
            .as_wide()
            .iter()
            .copied()
            .chain(std::iter::once(0))
            .collect();
        let mode_bytes: Vec<u8> = mode_str.iter().flat_map(|u| u.to_le_bytes()).collect();
        if BCryptSetProperty(BCRYPT_HANDLE(alg.0), BCRYPT_CHAINING_MODE, &mode_bytes, 0).is_err() {
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            return Err("设置 GCM 链模式失败".to_string());
        }
        if BCryptGenerateSymmetricKey(alg, &mut kh, None, key, 0).is_err() {
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            return Err("BCryptGenerateSymmetricKey 失败".to_string());
        }
        let mut done = 0u32;
        let iv = info as *const _ as *const core::ffi::c_void;
        let st = if encrypt {
            BCryptEncrypt(
                kh,
                Some(input),
                Some(iv),
                None,
                Some(out),
                &mut done,
                BCRYPT_FLAGS(0),
            )
        } else {
            BCryptDecrypt(
                kh,
                Some(input),
                Some(iv),
                None,
                Some(out),
                &mut done,
                BCRYPT_FLAGS(0),
            )
        };
        let _ = BCryptDestroyKey(kh);
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        if st.is_err() {
            return Err(if encrypt {
                "AES-GCM 加密失败".to_string()
            } else {
                "AES-GCM 解密失败（认证不通过：密钥、aad 或密文被改）".to_string()
            });
        }
        Ok(done)
    }
}
