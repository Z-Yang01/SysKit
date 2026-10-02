"""生成 M6 跨语言兼容测试用的 Python fixture。

用法：

    python tools/gen_vault_fixture.py

产出到 `%TEMP%\\sk_vault_xlang\\`：
- `vault.key`：**密码模式**，故意用 2000 次迭代（不是常量 600000），
  用来验证 Rust 是否按文件里的 `kdf.iter` 派生——这正是跨版本升级最容易踩的坑；
- `field_blob.b64`：固定 nonce、aad=`b"field"` 的密文，验证字段加解密互通；
- `expected.json`：密码与期望明文。

Rust 侧由 `vault_crypto::cross_language_tests` 读取；
反向（Rust 写 → Python 读）由 `tools/verify_vault_rust_to_python.py` 校验。
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from syskit import vault_crypto as vc

OUT = os.path.join(os.environ.get("TEMP", "/tmp"), "sk_vault_xlang")
PASSWORD = "correct horse battery staple"
PLAINTEXT = "跨语言往返测试 ✓"
ITER = 2000  # 故意不等于 KDF_ITER，逼 Rust 按文件读迭代数


def main() -> int:
    os.makedirs(OUT, exist_ok=True)

    # 固定 salt/key（便于失败时复现），但用真实的派生与封装函数
    salt = bytes(range(16))
    key = bytes(range(32))
    dk = vc._derive(PASSWORD, salt, ITER)
    meta = {
        "mode": "password",
        "kdf": {"salt": vc._b64(salt), "iter": ITER},
        "wrapped_key": vc._b64(vc._seal(dk, key, aad=b"vault-key")),
        "verifier": vc._b64(vc._seal(dk, vc._VERIFIER, aad=b"vault-verify")),
    }
    with open(os.path.join(OUT, "vault.key"), "w", encoding="utf-8") as f:
        json.dump(meta, f)

    # 固定 nonce 的字段密文（aad 必须是 b"field"）
    nonce = bytes([9] * 12)
    blob = nonce + vc._aesgcm(key).encrypt(nonce, PLAINTEXT.encode("utf-8"), b"field")
    with open(os.path.join(OUT, "field_blob.b64"), "w", encoding="utf-8") as f:
        f.write(vc._b64(blob))

    with open(os.path.join(OUT, "expected.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "password": PASSWORD,
                "key_hex": key.hex(),
                "field_plaintext": PLAINTEXT,
                "iter": ITER,
            },
            f,
            ensure_ascii=False,
        )

    # 自检：Python 必须能读回自己写的东西（否则 fixture 本身没意义）
    v = vc.VaultCrypto(os.path.join(OUT, "vault.key"))
    if not v.unlock(PASSWORD):
        print("自检失败：Python 无法解锁自己生成的 vault.key")
        return 1
    if v.decrypt(blob).decode("utf-8") != PLAINTEXT:
        print("自检失败：Python 无法解开自己生成的字段密文")
        return 1

    print(f"fixture 已生成并通过自检：{OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
