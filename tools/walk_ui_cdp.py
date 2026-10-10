"""CDP 驱动 SysKit 主窗交互走查：逐页点按钮、收集控制台错误、截图。

用法：python tools/walk_ui_cdp.py [页签名...]
"""
import json
import urllib.request
import websocket
import time
import base64
import os
import sys

TAGS = {
    'ov': '🏠 概览 / 网络监控',
    'clip': '📋 剪贴板历史',
    'diag': '🔍 诊断 / 修复',
    'tool': '⚡ 工具箱',
    'sys': '💼 系统助手',
    'vault': '🔒 密码库',
    'set': '⚙ 设置',
    'logs': '📜 日志',
}


def connect_main():
    pages = json.load(urllib.request.urlopen('http://127.0.0.1:9223/json'))
    for p in pages:
        if p.get('type') != 'page' or not p.get('webSocketDebuggerUrl'):
            continue
        w = websocket.create_connection(p['webSocketDebuggerUrl'], timeout=15,
                                        origin='http://127.0.0.1:9223')
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
            return w, call
        w.close()
    raise SystemExit('main window not found')


class Session:
    def __init__(self):
        self.ws, self._call = connect_main()
        self._id = [1000]
        self.errors = []

    def call(self, method, params=None):
        self._id[0] += 1
        self.ws.send(json.dumps({"id": self._id[0], "method": method, "params": params or {}}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("id") == self._id[0]:
                return m.get("result", m)

    def ev(self, expr, await_promise=False):
        r = self.call("Runtime.evaluate", {
            "expression": expr, "returnByValue": True, "awaitPromise": await_promise})
        res = r.get("result", {})
        if res.get("subtype") == 'error' or 'exceptionDetails' in r:
            d = r.get('exceptionDetails', {})
            return 'JSEXC: ' + str(d.get('exception', {}).get('description', d.get('text', '')))[:200]
        return res.get("value")

    def drain(self, secs=0.5):
        self.ws.settimeout(secs)
        try:
            while True:
                m = json.loads(self.ws.recv())
                if m.get("method") == "Runtime.exceptionThrown":
                    d = m["params"]["exceptionDetails"]
                    self.errors.append('EXC: ' + str(d.get('exception', {}).get('description', d.get('text', '')))[:200])
                elif m.get("method") == "Runtime.consoleAPICalled" and m["params"].get("type") == "error":
                    self.errors.append('ERR: ' + ' '.join(
                        str(a.get('value', a.get('description', '')))[:150] for a in m["params"].get('args', [])))
        except websocket.WebSocketTimeoutException:
            pass
        self.ws.settimeout(15)

    def start_errors(self):
        self.call("Runtime.enable")

    def click_nav(self, name):
        # button 一并匹配：v1.4 UI 迁移后侧栏是原生 button（AntD Menu 时代是 li）
        self.ev("(e=>{const els=[...document.querySelectorAll('li,div,span,button')].filter(x=>x.textContent?.trim()===" +
                json.dumps(name) + "&&x.children.length<=1);(els[els.length-1]||{click(){}}).click();})('')")
        time.sleep(1.6)
        self.drain(0.4)

    def click_text(self, text, nth=-1):
        """点击页面上含指定文本的叶子元素（按钮等）。"""
        r = self.ev("(e=>{const els=[...document.querySelectorAll('button,[role=button],a,.ant-btn,span,div')].filter(x=>x.textContent?.trim()===" +
                    json.dumps(text) + "&&x.offsetParent!==null);const t=els[nth];if(!t)return 'NOT-FOUND:'+els.length;t.click();return 'CLICKED';})('')".replace('nth', str(nth)))
        time.sleep(1.2)
        self.drain(0.4)
        return r

    def shot(self, name):
        shot = self.call("Page.captureScreenshot", {"format": "png"})
        tmp = os.environ.get('TEMP', r'C:\Users\y3030\AppData\Local\Temp')
        path = os.path.join(tmp, f'ui_{name}.png')
        with open(path, 'wb') as f:
            f.write(base64.b64decode(shot["data"]))
        return path

    def body(self):
        return self.ev("document.body.innerText") or ''


def main():
    s = Session()
    s.start_errors()
    want = sys.argv[1:] or list(TAGS.keys())
    for tag in want:
        name = TAGS[tag]
        before = len(s.errors)
        s.click_nav(name)
        path = s.shot(tag)
        b = s.body()
        new_err = s.errors[before:]
        print(f'== {name}: chars={len(b)} 新错误={len(new_err)} -> {path}')
        for e in new_err[:5]:
            print('   ', e[:170])
    return s


if __name__ == '__main__':
    main()
