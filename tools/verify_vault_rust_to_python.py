"""校验 Rust 版生成的 vault.key 能被 Python 版读取（反向跨语言兼容）。

用法（先跑 app-rs 里的 cross_language 测试生成产物）：

    python tools/verify_vault_rust_to_python.py

为什么需要这个脚本：正向（Python 写 → Rust 读）已由 Rust 单测覆盖，
但**反向**同样重要——用户从 Rust 版切回 Python 版时，若 Python 读不了
Rust 写的 vault.key，用户会看到「主密码错误」，甚至以为密码库损坏。
两边都验证过才算"格式兼容"。
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from syskit import vault_crypto as vc

FIXTURE = os.path.join(os.environ.get("TEMP", "/tmp"), "sk_vault_xlang", "rust_out")


def main() -> int:
    if not os.path.isdir(FIXTURE):
        print(f"跳过：未找到 Rust 产物目录 {FIXTURE}")
        print("请先运行： cargo test cross_language -- --nocapture")
        return 0

    key_path = os.path.join(FIXTURE, "vault.key")
    info = json.load(open(os.path.join(FIXTURE, "info.json"), encoding="utf-8"))
    blob = vc._unb64(open(os.path.join(FIXTURE, "field_blob.b64")).read())

    failures = []

    # 1) 元数据结构
    meta = json.load(open(key_path, encoding="utf-8"))
    for field in ("mode", "kdf", "wrapped_key", "verifier"):
        if field not in meta:
            failures.append(f"vault.key 缺少字段 {field}")
    if meta.get("mode") != "password":
        failures.append(f"mode 应为 password，实际 {meta.get('mode')!r}")
    kdf = meta.get("kdf") or {}
    if "salt" not in kdf or "iter" not in kdf:
        failures.append("kdf 缺少 salt/iter")

    # 2) 解锁
    v = vc.VaultCrypto(key_path)
    if not v.unlock(info["password"]):
        failures.append("Python 无法用同一密码解锁 Rust 生成的 vault.key")
    else:
        # 3) 解开 Rust 加密的字段
        try:
            plain = v.decrypt(blob).decode("utf-8")
        except Exception as e:  # noqa: BLE001 - 报告任何解密失败
            failures.append(f"Python 无法解开 Rust 生成的字段密文: {e}")
        else:
            if plain != info["field_plaintext"]:
                failures.append(
                    f"明文不一致：期望 {info['field_plaintext']!r}，得到 {plain!r}"
                )

        # 4) 错误密码必须被拒
        if v.verify_password(info["password"] + "x"):
            failures.append("错误密码竟然通过了校验")

    if failures:
        print("反向兼容校验失败：")
        for f in failures:
            print("  -", f)
        return 1

    print("反向兼容校验通过：Python 能读 Rust 生成的 vault.key 与字段密文。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
