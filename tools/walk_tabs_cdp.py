"""工具箱/系统助手子页 + 深色主题 全页走查。"""
import json
import urllib.request
import websocket
import time
import base64
import os


def connect(port):
    pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json'))
    for p in pages:
        if p.get('type') != 'page' or not p.get('webSocketDebuggerUrl'):
            continue
        w = websocket.create_connection(p['webSocketDebuggerUrl'], timeout=15,
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
errors = []


def ev(expr, awaitp=False):
    r = call("Runtime.evaluate", {"expression": expr, "returnByValue": True,
                                  "awaitPromise": awaitp})
    d = r.get('exceptionDetails')
    if d:
        errors.append('JSEXC: ' + str(d.get('exception', {}).get('description', d.get('text', '')))[:200])
        return None
    return r.get("result", {}).get('value')


def drain(secs=0.4):
    call("Runtime.evaluate", {"expression": "1", "returnByValue": True})
    ws = None
    # 拉取事件流里的 console 错误：走 CDP 事件需要单独 recv——简化为不拉
    return


def shot(name):
    shot = call("Page.captureScreenshot", {"format": "png"})
    tmp = os.environ.get('TEMP', r'C:\Users\y3030\AppData\Local\Temp')
    path = os.path.join(tmp, f'{name}.png')
    with open(path, 'wb') as f:
        f.write(base64.b64decode(shot["data"]))
    return path


def nav(name):
    ev(f"[...document.querySelectorAll('li,div,span')].filter(e=>e.textContent?.trim()==={json.dumps(name)}).pop()?.click()")
    time.sleep(1.4)


def click_tab(label):
    r = ev(f"[...document.querySelectorAll('.ant-tabs-tab')].filter(e=>e.textContent?.trim()==={json.dumps(label)}).pop()?.click()")
    time.sleep(1.4)
    return r is not None


def page_state():
    b = ev('document.body.innerText') or ''
    errs = ev('JSON.stringify(window.__errs||[])')
    return len(b), b


call("Runtime.enable")
call("Runtime.evaluate", {"expression": "window.__errs=[]; console.error=(...a)=>{window.__errs.push(a.join(' ').slice(0,200)); console.__err&&console.__err(...a)};", "returnByValue": True})

# 工具箱子页
nav('工具箱')
for tab, tag in [('Hosts 编辑', 'tool_hosts'), ('端口 / 局域网', 'tool_ports'),
                 ('DNS / 子网 / ping', 'tool_dns'), ('测速', 'tool_speed'),
                 ('应用联网', 'tool_appnet')]:
    click_tab(tab)
    n, b = page_state()
    print(f'工具箱[{tab}]: chars={n} errs={ev("window.__errs.length")}')
    shot(tag)

# 系统助手子页
nav('系统助手')
for tab, tag in [('启动项管理', 'sys_startup'), ('磁盘空间', 'sys_disk'),
                 ('垃圾清理', 'sys_clean'), ('电池健康', 'sys_battery'),
                 ('场景模式', 'sys_modes')]:
    click_tab(tab)
    n, b = page_state()
    print(f'系统助手[{tab}]: chars={n} errs={ev("window.__errs.length")}')
    shot(tag)

# 深色主题开
ev("[...document.querySelectorAll('button,.ant-switch')].filter(e=>e.closest('.ant-switch')||e.textContent?.trim()==='深色主题').forEach(e=>e.click())")
time.sleep(1.5)
nav('概览 / 网络监控')
n, b = page_state()
print('深色概览: chars=', n)
shot('dark_ov')
nav('剪贴板历史')
shot('dark_clip')
nav('密码库')
shot('dark_vault')
print('window errors:', ev('JSON.stringify(window.__errs)'))
