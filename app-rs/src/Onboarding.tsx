/**
 * 首次启动使用引导（4 步 stepper）。
 *
 * 触发：首次启动（localStorage 无标记）自动弹；设置页「查看使用引导」
 * 按钮派发 `sk:show-onboarding` 事件重看。关窗（含跳过/完成/X）一律记
 * 标记——引导只该打扰一次。
 *
 * 内容口径（对齐项目简约纪律）：每步一个图标 + 两行话，不截屏、不放
 * 动画——引导的价值是"知道有这条路"，不是替用户走一遍。
 */
import { useState, type ReactNode } from "react";
import { Modal } from "./ui/modal";
import { Button } from "./ui";

export const ONBOARDING_KEY = "syskit.onboarding.v1";

/** 是否已看过引导（localStorage 不可用时视为看过——引导是便利性，不是门槛）。 */
export function onboardingSeen(): boolean {
  try {
    return localStorage.getItem(ONBOARDING_KEY) === "1";
  } catch {
    return true;
  }
}

function markOnboardingSeen(): void {
  try {
    localStorage.setItem(ONBOARDING_KEY, "1");
  } catch {
    /* 同上：存不进就算了 */
  }
}

const STEPS: { icon: string; title: string; body: ReactNode }[] = [
  {
    icon: "🎛",
    title: "欢迎使用 SysKit",
    body: (
      <>
        <span>Windows 网络诊断与系统工具箱：诊断修复、性能监控、垃圾清理、密码库。</span>
        <span>
          关闭窗口后托盘常驻，后台继续监控网络——断网 / 恢复会弹通知。
          完全退出在托盘图标右键菜单里。
        </span>
      </>
    ),
  },
  {
    icon: "🌐",
    title: "网络出问题怎么办",
    body: (
      <>
        <span>
          打开「🌐 网络」页，点 <b>🔧 一键智能修复</b>：自动诊断 15 项、
          自动修复可自动处理的问题，全程不用懂网络术语。
        </span>
        <span>
          改配置类修复需要<b>管理员身份</b>（关窗后右键 SysKit.exe →
          以管理员身份运行）。所有修复前自动备份原值，可在「备份与回滚」一键还原。
        </span>
      </>
    ),
  },
  {
    icon: "⌘",
    title: "快速跳转",
    body: (
      <>
        <span>
          <code>Ctrl+K</code> 打开命令面板（全局热键 <code>Ctrl+Alt+K</code>）：
          搜页面、跑 <code>ping 主机</code> / <code>dns 域名</code>、触发快捷修复。
        </span>
        <span>
          <code>Ctrl+1~6</code> 直接切换六个页面。
        </span>
      </>
    ),
  },
  {
    icon: "🔒",
    title: "数据与隐私",
    body: (
      <>
        <span>
          所有数据<b>只存本机</b>：密码库 AES-256-GCM 加密落盘，剪贴板历史、
          诊断报告、网络记录不上传任何服务器。
        </span>
        <span>设置页顶部随时可以重看本引导。</span>
      </>
    ),
  },
];

export function Onboarding({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [step, setStep] = useState(0);
  const last = step === STEPS.length - 1;
  // 任何出口（跳过/完成/X）都记标记并复位步数——下次从设置页进来仍从头开始
  const close = () => {
    markOnboardingSeen();
    setStep(0);
    onClose();
  };
  const s = STEPS[step];
  return (
    <Modal
      open={open}
      title={`使用引导（${step + 1} / ${STEPS.length}）`}
      onClose={close}
      width={520}
      footer={
        <>
          <Button onClick={close}>跳过</Button>
          {step > 0 && <Button onClick={() => setStep((v) => v - 1)}>上一步</Button>}
          <Button variant="primary" onClick={() => (last ? close() : setStep((v) => v + 1))}>
            {last ? "开始使用" : "下一步"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-1">
        <div className="text-4xl leading-none">{s.icon}</div>
        <div className="text-base font-semibold">{s.title}</div>
        <div className="flex flex-col gap-1.5 text-sm leading-relaxed text-sk-sub [&_code]:rounded [&_code]:bg-sk-hover [&_code]:px-1">
          {s.body}
        </div>
        <div className="mt-1 flex gap-1.5">
          {STEPS.map((_, i) => (
            <span
              key={i}
              className="h-1.5 w-6 rounded-full transition-colors duration-200"
              style={{ background: i === step ? "var(--sk-accent)" : "var(--sk-border)" }}
            />
          ))}
        </div>
      </div>
    </Modal>
  );
}
