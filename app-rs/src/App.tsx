import { useState } from "react";
import { Button, ConfigProvider, Space, Switch, ThemeConfig, Typography } from "antd";
import { invoke } from "@tauri-apps/api/core";
import { applyTheme, palette, TOKEN_NAMES } from "./theme";

/** M0 壳：验证 Tauri↔React 通路 + 主题 token 双主题切换 + 后端命令调用。 */
export function App() {
  const [dark, setDark] = useState(false);
  const [pong, setPong] = useState("—");

  const antdTheme: ThemeConfig = {
    token: {
      colorPrimary: palette(dark).accent,
      colorBgBase: palette(dark).bg,
      colorBgContainer: palette(dark).card,
      colorText: palette(dark).text,
      colorBorder: palette(dark).border,
      borderRadius: 8,
    },
  };

  async function ping() {
    try {
      setPong(await invoke<string>("ping"));
    } catch (e) {
      setPong(`失败: ${String(e)}`);
    }
  }

  function toggle(darkNext: boolean) {
    setDark(darkNext);
    applyTheme(darkNext); // CSS 变量单源：组件取 --sk-*，切主题即换值
  }

  return (
    <ConfigProvider theme={antdTheme}>
      <div style={{ padding: 20, minHeight: "100vh" }}>
        <Space direction="vertical" size={16} style={{ width: "100%" }}>
          <Typography.Title level={4} style={{ margin: 0 }}>
            SysKit · Rust+Tauri 重写骨架（M0）
          </Typography.Title>

          <div className="sk-card">
            <Space>
              <span>深色主题</span>
              <Switch checked={dark} onChange={toggle} />
              <Button type="primary" onClick={ping}>
                调用 Rust ping
              </Button>
              <span className="sk-hint">后端应答: {pong}</span>
            </Space>
          </div>

          <div className="sk-card">
            <div style={{ marginBottom: 8 }} className="sk-hint">
              32 个语义 token（与 Python 版 theme.py 奇偶校验锁定）：
            </div>
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
              {TOKEN_NAMES.map((name) => (
                <span
                  key={name}
                  title={`--sk-${name}`}
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    gap: 4,
                    border: "1px solid var(--sk-border)",
                    borderRadius: 6,
                    padding: "2px 6px",
                    fontSize: 12,
                  }}
                >
                  <span
                    style={{
                      width: 12,
                      height: 12,
                      borderRadius: 3,
                      background: `var(--sk-${name})`,
                      display: "inline-block",
                    }}
                  />
                  {name}
                </span>
              ))}
            </div>
          </div>
        </Space>
      </div>
    </ConfigProvider>
  );
}
