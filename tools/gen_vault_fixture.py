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

    # DB fixture（M6 数据层）：真实 schema + 用同一数据密钥加密的字段，
    # 验证 Rust 侧 vault_store 对同一 vault.db 的互开与 CRUD 读。
    import sys as _sys
    _sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from syskit import vault as vault_mod

    db_path = os.path.join(OUT, "vault.db")
    if os.path.exists(db_path):
        os.remove(db_path)
    vault_mod._db_path = lambda: db_path          # 重定向到 fixture 目录
    vault_mod.init_db()
    site_id = vault_mod.add_site("Fixture Site", "fixture.example", "https://fixture.example",
                                 "别名甲，别名乙", "测试标签")
    acc1 = vault_mod.add_account(
        site_id, "db-password-密码1", v, label="主账号", username="alice",
        totp_secret="JBSWY3DPEHPK3PXP", notes="备注甲")
    acc2 = vault_mod.add_account(site_id, "db-password-2", v, label="备用", username="bob")
    vault_mod.toggle_favorite(acc2)
    expected_totp = vault_mod.totp_code("JBSWY3DPEHPK3PXP", at=59)
    assert expected_totp, "生成器自检：TOTP 密钥应有效"
    with open(os.path.join(OUT, "vault_db_expected.json"), "w", encoding="utf-8") as f:
        json.dump(
            {
                "site_name": "Fixture Site",
                "site_id": site_id,
                "acc1": acc1,
                "acc2": acc2,
                "acc1_password": "db-password-密码1",
                "acc1_totp_secret": "JBSWY3DPEHPK3PXP",
                "acc1_notes": "备注甲",
                "acc2_password": "db-password-2",
                "expected_totp_at59": expected_totp,
            },
            f, ensure_ascii=False)

    # 自检：按标准路径语义再读一遍
    assert vault_mod.list_sites()[0]["name"] == "Fixture Site"
    assert vault_mod.get_secret(acc1, v, "password") == "db-password-密码1"

    print(f"fixture 已生成并通过自检：{OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
