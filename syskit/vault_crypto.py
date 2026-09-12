"""密码库加密：主密码可选，密文落盘。

- 已设主密码：PBKDF2-HMAC-SHA256 派生密钥 → AES-256-GCM 包裹随机数据密钥，
  验证器（verifier）用于校验密码正确性。
- 未设主密码：随机数据密钥用 Windows DPAPI（CryptProtectData）包裹落盘，
  绑定当前系统用户，免密即用。
数据密钥只在解锁后驻留内存；encrypt/decrypt 在锁定态抛 VaultLockedError。
"""
import base64
import hashlib
import json
import os
import secrets
import sys

from . import config

KDF_ITER = 600_000          # PBKDF2 迭代次数（测试可 monkeypatch 调小）
_KEY_NAME = "SysKit vault key"
_VERIFIER = b"syskit-vault-ok"
_NONCE_LEN = 12


class VaultLockedError(RuntimeError):
    """密码库处于锁定状态，无法加解密。"""


# ---------------- DPAPI（stdlib ctypes） ----------------

def _dpapi_protect(data: bytes) -> bytes:
    import ctypes
    from ctypes import wintypes

    class BLOB(ctypes.Structure):
        _fields_ = [("cbData", wintypes.DWORD),
                    ("pbData", ctypes.POINTER(ctypes.c_char))]

    buf = ctypes.create_string_buffer(data, len(data))
    pin = BLOB(len(data), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char)))
    pout = BLOB()
    if not ctypes.windll.crypt32.CryptProtectData(
            ctypes.byref(pin), _KEY_NAME, None, None, None, 0, ctypes.byref(pout)):
        raise OSError("CryptProtectData 失败")
    try:
        return ctypes.string_at(pout.pbData, pout.cbData)
    finally:
        ctypes.windll.kernel32.LocalFree(pout.pbData)


def _dpapi_unprotect(blob: bytes) -> bytes:
    import ctypes
    from ctypes import wintypes

    class BLOB(ctypes.Structure):
        _fields_ = [("cbData", wintypes.DWORD),
                    ("pbData", ctypes.POINTER(ctypes.c_char))]

    buf = ctypes.create_string_buffer(blob, len(blob))
    pin = BLOB(len(blob), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char)))
    pout = BLOB()
    if not ctypes.windll.crypt32.CryptUnprotectData(
            ctypes.byref(pin), None, None, None, None, 0, ctypes.byref(pout)):
        raise OSError("CryptUnprotectData 失败")
    try:
        return ctypes.string_at(pout.pbData, pout.cbData)
    finally:
        ctypes.windll.kernel32.LocalFree(pout.pbData)


def _b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def _unb64(text: str) -> bytes:
    return base64.b64decode(text.encode("ascii"))


def _aesgcm(key: bytes):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    return AESGCM(key)


def _seal(key: bytes, plaintext: bytes, aad: bytes = b"") -> bytes:
    """AES-256-GCM：返回 nonce(12B) + ct。"""
    nonce = secrets.token_bytes(_NONCE_LEN)
    return nonce + _aesgcm(key).encrypt(nonce, plaintext, aad)


def _open_seal(key: bytes, blob: bytes, aad: bytes = b"") -> bytes:
    if len(blob) <= _NONCE_LEN:
        raise ValueError("密文长度非法")
    nonce, ct = blob[:_NONCE_LEN], blob[_NONCE_LEN:]
    return _aesgcm(key).decrypt(nonce, ct, aad)


def _derive(password: str, salt: bytes) -> bytes:
    return hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, KDF_ITER)


class VaultCrypto:
    """密码库密钥管理。生命周期：setup → (unlock ⇄ lock) → set/change/remove 主密码。"""

    def __init__(self, key_file: str = None):
        self._path = key_file or os.path.join(config.APPDATA_DIR, "vault.key")
        self._data_key = None          # 32B 数据密钥，仅解锁后驻留内存

    # ---- 状态 ----

    @property
    def is_provisioned(self) -> bool:
        return os.path.exists(self._path)

    @property
    def is_unlocked(self) -> bool:
        return self._data_key is not None

    def has_master_password(self) -> bool:
        return self.is_provisioned and self._meta().get("mode") == "password"

    def _meta(self) -> dict:
        try:
            with open(self._path, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            return {}

    def _write_meta(self, meta: dict):
        os.makedirs(os.path.dirname(self._path), exist_ok=True)
        tmp = self._path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(meta, f)
        os.replace(tmp, self._path)

    # ---- 生命周期 ----

    def setup(self, password: str = None) -> None:
        """首次初始化：生成数据密钥。有密码则密码包裹，否则 DPAPI 包裹。"""
        if self.is_provisioned:
            raise RuntimeError("密码库已初始化")
        self._provision_new(password)

    def _provision_new(self, password: str = None) -> None:
        key = secrets.token_bytes(32)
        if password:
            salt = secrets.token_bytes(16)
            dk = _derive(password, salt)
            meta = {
                "mode": "password",
                "kdf": {"salt": _b64(salt), "iter": KDF_ITER},
                "wrapped_key": _b64(_seal(dk, key, aad=b"vault-key")),
                "verifier": _b64(_seal(dk, _VERIFIER, aad=b"vault-verify")),
            }
        else:
            if sys.platform != "win32":
                raise RuntimeError("无主密码模式依赖 Windows DPAPI，请设置主密码")
            meta = {"mode": "dpapi", "wrapped_key": _b64(_dpapi_protect(key))}
        self._write_meta(meta)
        self._data_key = key

    def unlock(self, password: str = None) -> bool:
        """解锁：解包数据密钥到内存。成功返回 True（已解锁直接 True）。

        key 文件损坏/截断（掉电无 fsync）时返回 False 而非抛异常——
        否则 GUI 启动即崩，用户可能误删整个密码库。"""
        if self.is_unlocked:
            return True
        if not self.is_provisioned:
            return False
        try:
            meta = self._meta()
            wrapped = _unb64(meta["wrapped_key"])
        except Exception as e:
            from .logger import LOGGER
            LOGGER.error("vault.key 无法解析（可能已损坏）：{}。"
                         "密码库保持锁定；如确认放弃历史数据可删除该文件后重启。",
                         self._path)
            LOGGER.debug("vault.key 解析异常详情", exc_info=e)
            return False
        if meta.get("mode") == "dpapi":
            try:
                self._data_key = _dpapi_unprotect(wrapped)
                return True
            except Exception:
                return False
        kdf = meta.get("kdf", {})
        dk = _derive(password or "", _unb64(kdf.get("salt", "")))
        try:
            if _open_seal(dk, _unb64(meta["verifier"]), aad=b"vault-verify") != _VERIFIER:
                return False
            self._data_key = _open_seal(dk, wrapped, aad=b"vault-key")
            return True
        except Exception:
            return False

    def lock(self):
        self._data_key = None

    # ---- 主密码管理（均要求当前已解锁，因为要重包裹数据密钥） ----

    def set_master_password(self, password: str) -> None:
        """无主密码 → 设主密码；数据密钥不变。"""
        if not self.is_unlocked:
            raise VaultLockedError("请先解锁")
        if not password:
            raise ValueError("主密码不能为空")
        self._rewrap(lambda: self._provision_from_key(self._data_key, password))

    def verify_password(self, password: str) -> bool:
        """校验主密码（不改变解锁状态）。非密码模式返回 False。"""
        meta = self._meta()
        if meta.get("mode") != "password":
            return False
        kdf = meta.get("kdf", {})
        dk = _derive(password or "", _unb64(kdf.get("salt", "")))
        try:
            return _open_seal(dk, _unb64(meta["verifier"]), aad=b"vault-verify") == _VERIFIER
        except Exception:
            return False

    def remove_master_password(self, current: str) -> bool:
        """有主密码 → 移除（回到 DPAPI 模式）。当前密码错误返回 False。"""
        if not self.is_unlocked:
            raise VaultLockedError("请先解锁")
        if not self.has_master_password():
            return False
        if not self.verify_password(current):
            return False
        if sys.platform != "win32":
            raise RuntimeError("DPAPI 仅限 Windows")
        self._write_meta({"mode": "dpapi",
                          "wrapped_key": _b64(_dpapi_protect(self._data_key))})
        return True

    def change_master_password(self, new_password: str) -> None:
        self.set_master_password(new_password)

    def _provision_from_key(self, key: bytes, password: str) -> None:
        salt = secrets.token_bytes(16)
        dk = _derive(password, salt)
        meta = {
            "mode": "password",
            "kdf": {"salt": _b64(salt), "iter": KDF_ITER},
            "wrapped_key": _b64(_seal(dk, key, aad=b"vault-key")),
            "verifier": _b64(_seal(dk, _VERIFIER, aad=b"vault-verify")),
        }
        self._write_meta(meta)

    def _rewrap(self, fn):
        fn()

    # ---- 字段加解密 ----

    def encrypt(self, plaintext: bytes) -> bytes:
        if not self.is_unlocked:
            raise VaultLockedError("密码库未解锁")
        return _seal(self._data_key, plaintext, aad=b"field")

    def decrypt(self, blob: bytes) -> bytes:
        if not self.is_unlocked:
            raise VaultLockedError("密码库未解锁")
        return _open_seal(self._data_key, blob, aad=b"field")
