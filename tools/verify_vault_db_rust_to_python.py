"""校验 Rust 版写出的 vault.db + vault.key 能被 Python 版打开（M6 跨语言反向）。

前置：先跑 `cargo test vault_store`（rust_writes_vault_db_python_can_open
会在 %TEMP%\\sk_vault_xlang\\rust_db_out\\ 产出 vault.db / vault.key）。

用法：python tools/verify_vault_db_rust_to_python.py
校验四层：元数据形态 → 主密码解锁 → 站点/账号行读回 → 字段密文解密比对。
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from syskit import vault as vault_mod
from syskit import vault_crypto as vc

OUT = os.path.join(os.environ.get("TEMP", "/tmp"), "sk_vault_xlang", "rust_db_out")
PASSWORD = "rust-export-pw"


def main() -> int:
    db_path = os.path.join(OUT, "vault.db")
    key_path = os.path.join(OUT, "vault.key")
    if not (os.path.exists(db_path) and os.path.exists(key_path)):
        print("缺产物：先跑 cargo test vault_store（生成 rust_db_out/）")
        return 1

    # 1) 元数据形态
    meta = json.load(open(key_path, encoding="utf-8"))
    if meta.get("mode") != "password":
        print(f"mode 应为 password，实际 {meta.get('mode')!r}")
        return 1
    if int(meta.get("kdf", {}).get("iter", 0)) != vc.KDF_ITER:
        print(f"kdf.iter 应为 {vc.KDF_ITER}")
        return 1

    # 2) 主密码解锁（Rust 加密的 wrapped_key/verifier）
    v = vc.VaultCrypto(key_path)
    if not v.unlock(PASSWORD):
        print("Python 无法用导出密码解锁 Rust 写的 vault.key")
        return 1

    # 3) 行读回（重定向 db 路径到产物目录）
    vault_mod._db_path = lambda: db_path
    sites = vault_mod.list_sites()
    if len(sites) != 1 or sites[0]["name"] != "RustFixture":
        print(f"sites 读回异常：{sites}")
        return 1
    accs = vault_mod.list_accounts(sites[0]["id"])
    if len(accs) != 1 or accs[0]["username"] != "carol":
        print(f"accounts 读回异常：{accs}")
        return 1

    # 4) 字段密文解密（aad=b"field"，Rust 加密 → Python 解）
    plain = vault_mod.get_secret(accs[0]["id"], v, "password")
    if plain != "rust-写入-密码":
        print(f"密码字段解密不匹配：{plain!r}")
        return 1
    totp = vault_mod.get_secret(accs[0]["id"], v, "totp")
    if totp != "JBSWY3DPEHPK3PXP":
        print(f"TOTP 字段解密不匹配：{totp!r}")
        return 1

    print("跨语言反向校验通过：Rust 写的 vault.db/key 被 Python 完整读回")
    return 0


if __name__ == "__main__":
    sys.exit(main())
