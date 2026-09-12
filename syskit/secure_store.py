"""本地敏感文件加密存储：DPAPI 封装的 JSON 容器（透明迁移旧明文）。

容器格式：{"v": 1, "enc": "dpapi", "data": "<base64(密文)>"}
- 密封对象绑定当前 Windows 用户（CryptProtectData），换用户/换机器无法解密——
  对剪贴板历史这类本机便利数据是合适的保护级别。
- load_sealed_json：兼容旧明文文件（无 enc 字段）→ 返回原数据，下次 save 自动
  升级为密封格式；密封损坏（换用户/截断）→ 返回 default（与旧版损坏行为一致）。
- 密码库 secrets 本身已 AES-GCM 逐字段加密，不经过本模块（§23 密钥模型不动）。
"""
import base64
import json
import os

from .vault_crypto import _dpapi_protect, _dpapi_unprotect

MARKER = {"v": 1, "enc": "dpapi"}


def seal_json(data) -> str:
    """序列化 → DPAPI 密封 → JSON 容器文本（可直接写文件）。"""
    raw = json.dumps(data, ensure_ascii=False).encode("utf-8")
    return json.dumps({**MARKER, "data": base64.b64encode(_dpapi_protect(raw)).decode("ascii")})


def load_sealed_json(path: str, default):
    """读取密封文件；兼容旧明文；损坏/换用户 → default。返回 (data, was_sealed)。"""
    try:
        with open(path, "r", encoding="utf-8") as f:
            parsed = json.load(f)
    except Exception:
        return default, False
    if isinstance(parsed, dict) and parsed.get("enc") == "dpapi":
        try:
            raw = _dpapi_unprotect(base64.b64decode(parsed.get("data", "")))
            return json.loads(raw.decode("utf-8")), True
        except Exception:
            return default, False
    return parsed, False            # 旧明文：原样返回，下次保存自动密封


def save_sealed_json(path: str, data) -> bool:
    """密封写入（原子替换）。成功返回 True。"""
    tmp = path + ".tmp"
    try:
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(seal_json(data))
        os.replace(tmp, path)
        return True
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        return False
