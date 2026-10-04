"""功能全扫：真实调用每个用户可见的后端命令，报告 OK/ERR。"""
import json
import urllib.request
import websocket

def connect(port):
    pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json'))
    for p in pages:
        if p.get('type') != 'page' or not p.get('webSocketDebuggerUrl'):
            continue
        w = websocket.create_connection(p['webSocketDebuggerUrl'], timeout=60,
                                        origin=f'http://127.0.0.1:{port}')
        _id = [0]

        def call(method, params=None, _w=w, _i=_id):
            _i[0] += 1
            _w.send(json.dumps({"id": _i[0], "method": method, "params": params or {}}))
            while True:
                m = json.loads(_w.recv())
                if m.get("id") == _i[0]:
                    return m.get("result", m)

        label = call("Runtime.evaluate", {
            "expression": "(window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label)||'?'",
            "returnByValue": True})["result"]["value"]
        if label == 'main':
            return call
        w.close()
    raise SystemExit('nf')


call = connect(9223)


def inv(cmd, args=None, timeout=45):
    expr = (f"window.__TAURI_INTERNAL__.invoke('{cmd}'"
            if False else
            f"window.__TAURI_INTERNALS__.invoke('{cmd}'"
            f"{(', ' + json.dumps(args)) if args else ''})"
            f".then(v=>JSON.stringify(v).slice(0,160)).catch(e=>'ERR:'+String(e).slice(0,120))")
    r = call("Runtime.evaluate", {"expression": expr, "returnByValue": True,
                                  "awaitPromise": True})
    d = r.get('exceptionDetails')
    if d:
        return 'JSEXC: ' + str(d.get('exception', {}).get('description', d.get('text', '')))[:120]
    return r.get("result", {}).get('value')


CASES = [
    ('ping 后端', 'ping', None),
    ('DNS 解析', 'sk_dns_query', {'host': 'www.baidu.com'}),
    ('子网计算', 'sk_subnet_calc', {'cidr': '192.168.1.0/24'}),
    ('ping 工具', 'sk_ping_host', {'host': '223.5.5.5', 'count': 2}),
    ('端口扫描', 'sk_port_scan', {'host': '127.0.0.1', 'ports': [80, 135, 445, 3389], 'timeoutMs': 400}),
    ('局域网设备', 'sk_lan_scan', {}),
    ('Hosts 读取', 'sk_hosts_read', {}),
    ('诊断-快速检测', 'sk_netcheck_run', {'quick': True}),
    ('修复预览', 'sk_monitor_preview', {}),
    ('监控状态', 'sk_monitor_state', {}),
    ('启动项列表', 'sk_startup_list', {}),
    ('磁盘空间', 'sk_disk_usage', {}),
    ('清理扫描', 'sk_cleanup_scan', {}),
    ('电池状态', 'sk_battery_status', {}),
    ('场景模式列表', 'sk_modes_list', {}),
    ('WiFi 列表', 'sk_wifi_profiles', {}),
    ('测速能力', 'sk_speedtest_status', {}),
    ('剪贴板概要', 'sk_clip_summary', {}),
    ('剪贴板探测', 'sk_clip_probe', {}),
    ('应用联网-校验', 'sk_appnet_validate', {'program': r'C:\Windows\notepad.exe'}),
    ('vault 状态', 'sk_vault_status', {}),
    ('设置全量', 'sk_settings_all', {}),
    ('版本', 'sk_version', {}),
]

print(f'{"功能":<14}{"结果"}')
print('-' * 90)
bad = []
for name, cmd, args in CASES:
    try:
        r = inv(cmd, args)
    except Exception as e:
        r = f'EXC: {e}'
    ok = r and not str(r).startswith(('ERR:', 'JSEXC:')) and 'missing required' not in str(r)
    mark = 'OK ' if ok else 'BAD'
    if not ok:
        bad.append((name, r))
    print(f'{mark} {name:<12}{str(r)[:110]}')

print()
print(f'=== 坏的 {len(bad)} 个 ===')
for name, r in bad:
    print(' ', name, '->', str(r)[:160])
