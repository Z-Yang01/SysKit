/** M6 数据层界面模型：行标题/掩码/TOTP 倒计时/阶段门。 */
import { describe, expect, it } from "vitest";
import {
  accountTitle,
  canAddPassword,
  maskedSecret,
  totpRemainingSecs,
  vaultPhase,
  type AccountRow,
  type VaultStatus,
} from "../src/vault";

function status(partial: Partial<VaultStatus>): VaultStatus {
  return {
    path: "k",
    provisioned: true,
    unlocked: true,
    has_master_password: false,
    mode: "dpapi",
    capabilities: { key_management: true, password_storage: true },
    ...partial,
  };
}

function account(partial: Partial<AccountRow>): AccountRow {
  return {
    id: 1,
    site_id: 1,
    label: "",
    username: "",
    is_favorite: false,
    has_totp: false,
    has_notes: false,
    ...partial,
  };
}

describe("account title", () => {
  it("label 优先，退用户名，再退 id", () => {
    expect(accountTitle(account({ label: "主账号", username: "alice" }))).toBe("主账号");
    expect(accountTitle(account({ username: "alice" }))).toBe("alice");
    expect(accountTitle(account({ id: 7 }))).toBe("账号 #7");
  });

  it("空白 label 视为未填", () => {
    expect(accountTitle(account({ label: "  ", username: "bob" }))).toBe("bob");
  });
});

describe("maskedSecret", () => {
  it("固定长度掩码（不外泄密文长度）", () => {
    expect(maskedSecret()).toBe("••••••••");
    expect(maskedSecret()).toBe(maskedSecret());
  });
});

describe("totpRemainingSecs", () => {
  it("落在 [1,30] 区间", () => {
    for (const ms of [0, 59_500, 60_000, 173_000]) {
      const r = totpRemainingSecs(ms);
      expect(r).toBeGreaterThanOrEqual(0);
      expect(r).toBeLessThanOrEqual(30);
    }
    // 29.5s 处 → 30-29 = 1
    expect(totpRemainingSecs(29_500)).toBe(1);
    // 整分界 60_000ms → 30-0 = 30
    expect(totpRemainingSecs(60_000)).toBe(30);
  });
});

describe("capability gate", () => {
  it("数据层可用 + 未解锁 → 添加被拒且理由指向解锁", () => {
    const r = canAddPassword(status({ unlocked: false }));
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("解锁");
  });

  it("数据层可用 + 已解锁 → 可添加", () => {
    expect(canAddPassword(status({ unlocked: true })).ok).toBe(true);
  });

  it("phase 三态", () => {
    expect(vaultPhase(null)).toBe("unprovisioned");
    expect(vaultPhase(status({ provisioned: false }))).toBe("unprovisioned");
    expect(vaultPhase(status({ unlocked: false }))).toBe("locked");
    expect(vaultPhase(status({ unlocked: true }))).toBe("unlocked");
  });
});
