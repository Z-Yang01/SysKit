"""功能全扫 v2：用真实命令名/签名重跑。"""
import json
import urllib.request
import websocket

def connect(port):
    pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json'))
    for p in pages:
        if p.get('type') != 'page' or not p.get('webSocketDebuggerUrl'):
            continue
        w = websocket.create_connection(p['webSocketDebuggerUrl'], timeout=90,
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


def inv(cmd, args=None):
    expr = (f"window.__TAURI_INTERNALS__.invoke('{cmd}'"
            f"{(', ' + json.dumps(args)) if args else ''})"
            f".then(v=>JSON.stringify(v).slice(0,150)).catch(e=>'ERR:'+String(e).slice(0,110))")
    r = call("Runtime.evaluate", {"expression": expr, "returnByValue": True,
                                  "awaitPromise": True})
    d = r.get('exceptionDetails')
    if d:
        return 'JSEXC: ' + str(d.get('exception', {}).get('description', d.get('text', '')))[:110]
    return r.get("result", {}).get('value')


CASES = [
    ('DNS 解析', 'sk_dns_query', {'domain': 'www.baidu.com'}),
    ('DNS 指定服务器', 'sk_dns_query', {'domain': 'www.qq.com', 'server': '223.5.5.5'}),
    ('子网计算', 'sk_subnet_calc', {'ip': '192.168.1.66', 'prefix': 24}),
    ('ping 工具', 'sk_ping', {'host': '223.5.5.5', 'count': 2}),
    ('端口扫描页命令', 'sk_port_probe', {'host': '127.0.0.1'}),
    ('诊断-快速检测', 'sk_diag_run', {'quick': True}),
    ('磁盘列表', 'sk_disk_list', {}),
    ('清理扫描', 'sk_clean_scan', {}),
    ('电池健康', 'sk_battery_health', {}),
    ('场景模式', 'sk_modes_info', {}),
    ('WiFi 信息', 'sk_wifi_info', {}),
    ('DNS 测速', 'sk_dns_speed', None),
]

bad = []
print(f'{"功能":<14}结果')
print('-' * 92)
for name, cmd, args in CASES:
    try:
        r = inv(cmd, args)
    except Exception as e:
        r = f'EXC: {e}'
    ok = r and not str(r).startswith(('ERR:', 'JSEXC:')) and 'missing required' not in str(r) and 'not found' not in str(r)
    mark = 'OK ' if ok else 'BAD'
    if not ok:
        bad.append((name, r))
    print(f'{mark} {name:<12}{str(r)[:118]}')

print()
print(f'=== 坏的 {len(bad)} 个 ===')
for name, r in bad:
    print(' ', name, '->', str(r)[:150])
