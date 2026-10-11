# -*- coding: utf-8 -*-
"""简约化提示文案清扫：实现细节长段删除/收拢为短句（r-string 防转义）。"""
import io
import os

os.chdir(os.path.dirname(os.path.abspath(__file__)) + "/../app-rs/src")


def patch(p, pairs):
    s = io.open(p, encoding="utf-8").read()
    ok = True
    for old, new in pairs:
        if old not in s:
            print(f"MISS in {p}: {old[:60]!r}")
            ok = False
            continue
        s = s.replace(old, new, 1)
    io.open(p, "w", encoding="utf-8", newline="\n").write(s)
    if ok:
        print("OK", p)


# ---- SystemPage：删「采样窗口」文档卡 + 两条来源说明 ----
patch(
    "SystemPage.tsx",
    [
        (
            r'''          <span className="text-[15px] font-semibold text-sk-text">WiFi 状态</span>
          <span className="sk-hint">每 5 秒刷新 · 数据来自 wlanapi.dll</span>''',
            r'''          <span className="text-[15px] font-semibold text-sk-text">WiFi 状态</span>''',
        ),
        (
            r'''          <div className="sk-hint mt-2">
            数据源为 <code>tasklist /fo csv</code>（约 0.5 秒/次，后端 2 秒缓存）。
            占比分母是**进程内存合计**而不是物理内存——tasklist 报的是工作集，合计会大于物理内存。
          </div>''',
            "",
        ),
        (
            r'''      <Card>
        <div className="px-4 pb-4 pt-3">
          <div className="sk-hint">
            曲线保留最近 {SERIES_LEN} 个采样点（1 秒/点，约 3 分钟）。页面隐藏期间采样会暂停，
            切回时若断流超过 10 分钟自动清空旧序列——避免把两段不连续的数据画成一条连续曲线。
            {last && <> 当前：<Badge>{wifiSummary(wifi)}</Badge></>}
          </div>
        </div>
      </Card>
''',
            "",
        ),
    ],
)

# ---- ToolboxClipboard：两条长提示收拢 ----
patch(
    "ToolboxClipboard.tsx",
    [
        (
            r'''        <div className="sk-hint">
          点击行即可复制回剪贴板（写回不会记进历史）；操作按钮在行悬停时浮现。置顶条目不会被自动淘汰，且相同文本再次复制时不会新增条目。
        </div>''',
            r'''        <div className="sk-hint">点击行复制；悬停行浮现操作。</div>''',
        ),
        (
            r'''              <div className="sk-hint">
                标准 WiFi 二维码格式，手机相机扫码即可加入网络。编码串包含明文密码，本页只渲染成图、
                不显示串本身——也别把二维码发给不该有网络权限的人。
              </div>''',
            r'''              <div className="sk-hint">编码串含明文密码：只渲染成图，别发给不该给的人。</div>''',
        ),
    ],
)

# ---- ToolboxSpeed：删串行探测说明块，收拢下载提示 ----
patch(
    "ToolboxSpeed.tsx",
    [
        (
            r'''      <Alert
        tone="info"
        title="每台服务器串行探测（避免自我并发干扰），服务器之间并发；单次超时按失败计入、不重发——测速要的是真实一次往返的表现。"
      />

''',
            "",
        ),
        (
            r'''            <span className="sk-hint">
              {status?.urls.length ?? 3} 个源 × 8 秒，下载即断不落盘（约消耗几十 MB 流量）
            </span>''',
            r'''            <span className="sk-hint">{status?.urls.length ?? 3} 源 × 8 秒 · 约几十 MB 流量</span>''',
        ),
        (
            r'''        {busy && <span className="sk-hint">测速中… 已用时 {elapsed} 秒</span>}''',
            r'''        {busy && <span className="sk-hint">已用时 {elapsed} 秒</span>}''',
        ),
    ],
)

# ---- ToolboxHosts：页脚长段收拢 ----
patch(
    "ToolboxHosts.tsx",
    [
        (
            r'''      <div className="sk-hint">
        禁用不会删除程序，只是让它下次登录时不自启；禁用记录存在
        <code>HKCU\Software\NetTool\DisabledRun</code>（与 Python 版共用），随时可恢复。
        启动文件夹项禁用后文件名会加 <code>.disabled</code> 后缀。
      </div>''',
            r'''      <div className="sk-hint">禁用只是不自启，可随时恢复；写回前自动备份原值。</div>''',
        ),
    ],
)

# ---- WifiPasswordPanel：页脚与图例收拢 ----
patch(
    "WifiPasswordPanel.tsx",
    [
        (
            r'''      <div className="sk-hint">
        提示：Win11 扫描周边 WiFi 需要系统「位置」服务开启（设置→隐私和安全性→位置）。
        扫描走 netsh，双频合一的网络按 BSSID 拆成多行（2.4G / 5G 各一条）。
      </div>''',
            r'''      <div className="sk-hint">扫不到网络？Win11 需开启「位置」服务；双频网络按 BSSID 拆行。</div>''',
        ),
        (
            r'''      <div className="sk-hint">
        条长 = 该信道 AP 数占本频段最挤信道的比例；颜色：绿 ≤2 个、橙 ≤4 个、红更多。
        推荐 2.4G 信道计入 ±4 内邻频重叠与信号强弱；5G 按 80MHz 块挑最空锚点并避开 DFS 雷达信道。
      </div>''',
            r'''      <div className="sk-hint">条长 = 占最挤信道比例 · 绿 ≤2 · 橙 ≤4 · 红 更多</div>''',
        ),
        (
            r'''              <div className="sk-hint">
                标准 WiFi 二维码格式，手机相机扫码即可加入网络。编码串包含明文密码，本页只渲染成图、
                不显示串本身——也别把二维码发给不该有网络权限的人。
              </div>''',
            r'''              <div className="sk-hint">编码串含明文密码：只渲染成图，别发给不该给的人。</div>''',
        ),
    ],
)

# ---- ToolboxQr：提示收拢 ----
patch(
    "ToolboxQr.tsx",
    [
        (
            r'''        <span className="sk-hint">支持截图 / 图片文件里的二维码；识别在本机完成，不联网</span>''',
            r'''        <span className="sk-hint">本机识别，不联网</span>''',
        ),
    ],
)

# ---- ToolboxDisk：磁盘页脚整段删除 ----
patch(
    "ToolboxDisk.tsx",
    [
        (
            r'''      <div className="sk-hint">
        「可用」按**整卷空闲**计算（不是当前用户配额内的可用量）——前者才是"这个盘还剩多少"。
        可移动盘不参与空间告警。
      </div>
''',
            "",
        ),
    ],
)

# ---- ToolboxStartup：页脚收拢 ----
patch(
    "ToolboxStartup.tsx",
    [
        (
            r'''      <div className="sk-hint">
        禁用不会删除程序，只是让它下次登录时不自启；禁用记录存在
        <code>HKCU\Software\NetTool\DisabledRun</code>（与 Python 版共用），随时可恢复。
        启动文件夹项禁用后文件名会加 <code>.disabled</code> 后缀。
      </div>''',
            r'''      <div className="sk-hint">禁用只是不自启，可随时恢复；写回前自动备份。</div>''',
        ),
    ],
)

# ---- Diagnose：内部参照文案删除/收拢 ----
patch(
    "Diagnose.tsx",
    [
        (
            r'''          <span className="sk-hint">按 Python COMBOS 的定义逐步执行</span>''',
            "",
        ),
        (
            r'''              <span className="sk-hint font-normal">动作清单（表格视图，顺序与后端 ACTION_IDS 一致）</span>''',
            r'''              <span className="sk-hint font-normal">动作清单</span>''',
        ),
    ],
)

# ---- BackupRollbackCard：说明收拢 ----
patch(
    "BackupRollbackCard.tsx",
    [
        (
            r'''        <div className="sk-hint mb-2">
          每次修复动作改配置前都会先备份原值。这里按会话列出，可一键还原结构化项
          （系统代理 / DNS / NCSI 注册表）。
        </div>''',
            r'''        <div className="sk-hint mb-2">修复前的原值备份，可一键还原。</div>''',
        ),
    ],
)

# ---- Overview：空态文案收拢 ----
patch(
    "Overview.tsx",
    [
        (
            r'''                        events.length === 0
                          ? "暂无事件——网络状态切换、质量告警、流量提醒都会写入这里"
                          : "该类型暂无事件"''',
            r'''                        events.length === 0 ? "暂无事件" : "该类型暂无事件"''',
        ),
    ],
)

# ---- ToolboxAppNet：扫描提示收拢 ----
patch(
    "ToolboxAppNet.tsx",
    [
        (
            r'''      <UiAlert tone="info" title="扫描本机所在 /24 网段（并发 ping 唤醒邻居后读 ARP 表），约需 8~15 秒" />''',
            r'''      <UiAlert tone="info" title="扫描 /24 网段，约需 8~15 秒" />''',
        ),
    ],
)

print("done")
