"""Helpers for the browser tests: a small Chrome DevTools client (standard
library only), a headless Chrome, `diffnote serve`, and reviews to open.

The reviews are made with the real `diffnote` (its `edit` run with a fake
editor, see `fake_editor.py`), so the tests need only Python, Chrome and a
built `diffnote` (set DIFFNOTE_BIN, or `cargo build` for target/debug).
"""
import base64
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
EXE = "diffnote.exe" if os.name == "nt" else "diffnote"
BIN = os.environ.get("DIFFNOTE_BIN") or os.path.join(ROOT, "target", "debug", EXE)


def find_chrome():
    for name in (os.environ.get("CHROME"), "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"):
        if name and shutil.which(name):
            return shutil.which(name)
    return None


def require_environment():
    """Skip the module when the browser or the binary is missing."""
    if not find_chrome():
        raise unittest.SkipTest("no Chrome/Chromium (set CHROME)")
    if not os.path.exists(BIN):
        raise unittest.SkipTest(f"{BIN} not built (cargo build, or set DIFFNOTE_BIN)")


# ---- Chrome DevTools -------------------------------------------------------

class Cdp:
    """Evaluate JavaScript in a page and send input events."""

    def __init__(self, port):
        info = None
        for _ in range(100):
            try:
                info = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json"))
                if any(t["type"] == "page" for t in info):
                    break
            except Exception:
                time.sleep(0.1)
        ws = next(t for t in info if t["type"] == "page")["webSocketDebuggerUrl"]
        host, path = ws[5:].split("/", 1)
        h, p = host.split(":")
        self.sock = socket.create_connection((h, int(p)))
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f"GET /{path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            buf += self.sock.recv(1)
        self.next_id = 0
        self.buf = b""

    def _send(self, text):
        data = text.encode()
        n = len(data)
        mask = os.urandom(4)
        head = bytes([0x81])
        if n < 126:
            head += bytes([0x80 | n])
        elif n < 65536:
            head += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            head += bytes([0x80 | 127]) + struct.pack(">Q", n)
        self.sock.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _take(self, k):
        while len(self.buf) < k:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError
            self.buf += chunk
        out, self.buf = self.buf[:k], self.buf[k:]
        return out

    def _receive(self):
        message = b""
        while True:
            b1, b2 = self._take(2)
            n = b2 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._take(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._take(8))[0]
            message += self._take(n)
            if b1 & 0x80:
                return message.decode()

    def call(self, method, **params):
        self.next_id += 1
        mid = self.next_id
        self._send(json.dumps({"id": mid, "method": method, "params": params}))
        while True:
            m = json.loads(self._receive())
            if m.get("id") == mid:
                return m

    def js(self, expression):
        r = self.call("Runtime.evaluate", expression=expression, awaitPromise=True, returnByValue=True)
        result = r.get("result", {})
        if "exceptionDetails" in result:
            raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False)[:500])
        return result.get("result", {}).get("value")

    def wait(self, expression, timeout=8):
        """Wait until `expression` is truthy. (A DOM node does not come back
        by value: wrap it in `!!`.)"""
        end = time.time() + timeout
        while time.time() < end:
            if self.js(expression):
                return True
            time.sleep(0.03)
        return False

    def mouse(self, kind, x, y, buttons=0, modifiers=0):
        self.call("Input.dispatchMouseEvent", type=kind, x=x, y=y, button="left",
                  buttons=buttons, clickCount=1, modifiers=modifiers)

    def key(self, key, code, vk):
        self.call("Input.dispatchKeyEvent", type="keyDown", key=key, code=code, windowsVirtualKeyCode=vk)


class Browser:
    """A headless Chrome with one page, and helpers to look at it."""

    def __init__(self):
        self.profile = tempfile.mkdtemp(prefix="dn-chrome-")
        self.proc = subprocess.Popen(
            [find_chrome(), "--headless=new", "--no-sandbox", "--disable-gpu", "--remote-debugging-port=0",
             f"--user-data-dir={self.profile}", "--window-size=1500,900", "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        port_file = os.path.join(self.profile, "DevToolsActivePort")
        for _ in range(200):
            if os.path.exists(port_file):
                break
            time.sleep(0.05)
        with open(port_file) as f:
            port = int(f.readline())
        self.cdp = Cdp(port)
        self.cdp.call("Page.enable")

    def close(self):
        self.cdp.sock.close()
        self.proc.terminate()
        self.proc.wait(timeout=10)
        shutil.rmtree(self.profile, ignore_errors=True)

    # -- page --
    def open(self, url, ready="!!document.querySelector('.diffnote-diff')"):
        self.cdp.call("Page.navigate", url=url)
        assert self.cdp.wait(f"document.readyState==='complete' && {ready}"), f"{url} did not load"

    def reload(self, ready="!!document.querySelector('.diffnote-diff')"):
        self.cdp.call("Page.reload")
        assert self.cdp.wait(f"document.readyState==='complete' && {ready}")

    def js(self, expression):
        return self.cdp.js(expression)

    def wait(self, expression, timeout=8):
        return self.cdp.wait(expression, timeout)

    def visible(self, selector):
        """How many elements matching `selector` are displayed."""
        return self.js("Array.from(document.querySelectorAll(%s)).filter(function(e){return e.getClientRects().length>0}).length" % json.dumps(selector))

    # Looking at the page by CSS selector, without quoting worries.
    def count(self, selector):
        return self.js("document.querySelectorAll(%s).length" % json.dumps(selector))

    def exists(self, selector):
        return self.js("!!document.querySelector(%s)" % json.dumps(selector))

    def text(self, selector):
        return self.js("document.querySelector(%s).textContent" % json.dumps(selector))

    def value(self, selector):
        return self.js("document.querySelector(%s).value" % json.dumps(selector))

    def wait_exists(self, selector, timeout=8):
        return self.wait("!!document.querySelector(%s)" % json.dumps(selector), timeout)

    def wait_gone(self, selector, timeout=8):
        return self.wait("!document.querySelector(%s)" % json.dumps(selector), timeout)

    def wait_count(self, selector, n, timeout=8):
        return self.wait("document.querySelectorAll(%s).length === %d" % (json.dumps(selector), n), timeout)

    def set_value(self, selector, text):
        self.js("document.querySelector(%s).value = %s" % (json.dumps(selector), json.dumps(text)))

    def click(self, selector):
        self.js("document.querySelector(%s).click()" % json.dumps(selector))

    def center(self, selector):
        return self.js("(function(){var e=document.querySelector(%s); e.scrollIntoView({block:'center'}); var r=e.getBoundingClientRect(); return [r.x+r.width/2, r.y+r.height/2]})()" % json.dumps(selector))

    def press(self, selector, modifiers=0):
        x, y = self.center(selector)
        self.cdp.mouse("mouseMoved", x, y)
        self.cdp.mouse("mousePressed", x, y, 1, modifiers)
        return x, y

    def release(self, selector=None, modifiers=0):
        x, y = self.center(selector) if selector else (0, 0)
        self.cdp.mouse("mouseReleased", x, y, 0, modifiers)

    def click_at(self, selector, modifiers=0):
        self.press(selector, modifiers)
        self.release(selector, modifiers)

    def drag(self, first, last):
        x, y = self.press(first)
        x2, y2 = self.center(last)
        self.cdp.mouse("mouseMoved", x2, y2, 1)
        self.release(last)

    def hover(self, selector):
        x, y = self.center(selector)
        self.cdp.mouse("mouseMoved", x, y)

    def escape(self):
        self.cdp.key("Escape", "Escape", 27)

    def screenshot(self, path):
        data = self.cdp.call("Page.captureScreenshot", format="png")["result"]["data"]
        with open(path, "wb") as f:
            f.write(base64.b64decode(data))

    def stub_clipboard(self):
        self.js("window.__copied=null; Object.defineProperty(navigator,'clipboard',{value:{writeText:function(t){window.__copied=t;return Promise.resolve();}},configurable:true})")


# ---- diffnote ----------------------------------------------------------------

def diffnote(*args, cwd=None, env=None, comments=None):
    """Run `diffnote`; `comments` are (after-line, text) pairs for the fake editor."""
    full_env = dict(os.environ)
    if env:
        full_env.update(env)
    if comments is not None:
        script = tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False, encoding="utf-8")
        script.write("".join(f"{a}\t{t}\n" for a, t in comments))
        script.close()
        full_env["DN_SCRIPT"] = script.name
        full_env["EDITOR"] = f'"{sys.executable}" "{os.path.join(HERE, "fake_editor.py")}"'
    out = subprocess.run([BIN, *args], cwd=cwd, env=full_env, capture_output=True, text=True, encoding="utf-8")
    if comments is not None:
        os.unlink(script.name)
    return out


def show(review):
    return diffnote("show", "-f", review).stdout


def entries(review):
    """The number of events in the review's log."""
    out = subprocess.run(["unzip", "-p", review, "review.jsonl"], capture_output=True, text=True, encoding="utf-8")
    return len(out.stdout.splitlines())


def zip_names(review):
    return subprocess.run(["unzip", "-Z1", review], capture_output=True, text=True).stdout.split()


class Served:
    """`diffnote serve` on a review."""

    def __init__(self, review, cwd=None, extra=(), author="tester"):
        self.review = review
        self.proc = subprocess.Popen([BIN, "serve", "-f", review, "--no-open", "--author", author, *extra],
                                     cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8")
        self.notices = []
        self.url = None
        for line in self.proc.stdout:
            if "注意" in line:
                self.notices.append(line.strip())
            m = re.search(r"(http://127\.0\.0\.1:\d+/\?t=\w+)", line)
            if m:
                self.url = m.group(1)
                break
        assert self.url, "diffnote serve did not start"

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
        self.proc.wait(timeout=10)
        self.proc.stdout.close()

    def shut_down(self, browser):
        """End the server the way the page's button does, and wait for it."""
        browser.js("fetch('/api/shutdown',{method:'POST',headers:{'X-Diffnote':'1'}})")
        self.proc.wait(timeout=10)


# ---- reviews to open ------------------------------------------------------------

def git(repo, *args):
    out = subprocess.run(["git", "-C", repo, "-c", "user.email=t@example.com", "-c", "user.name=T", *args],
                         capture_output=True, text=True, encoding="utf-8")
    assert out.returncode == 0, f"git {args}: {out.stderr}"
    return out.stdout.strip()


CALC_V1 = "def add(a, b):\n    return a + b\n\n\ndef div(a, b):\n    return a / b\n"
CALC_V2 = ("def add(a, b):\n    return a + b\n\n\ndef div(a, b):\n    if b == 0:\n        return None\n"
           "    return a / b\n\n\ndef mul(a, b):\n    return a * b\n")
CALC_V3 = '"""calc"""\n\n' + CALC_V2.replace("return None", 'raise ValueError("b is zero")')


def make_calc_review(root):
    """A git review with two revisions of the same base (c1..c2, then c1..c3),
    a review-wide thread, threads on `return None` (resolved) and on `mul`, and
    a thread on README.md (which the second revision's diff doesn't touch...
    the first one's doesn't either), by the real `edit`."""
    repo = os.path.join(root, "calc")
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    write(repo, "calc.py", CALC_V1)
    write(repo, "README.md", "# calc\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "c1")
    git(repo, "tag", "c1")
    write(repo, "calc.py", CALC_V2)
    git(repo, "commit", "-q", "-am", "c2")
    git(repo, "tag", "c2")
    write(repo, "calc.py", CALC_V3)
    git(repo, "commit", "-q", "-am", "c3")
    git(repo, "tag", "c3")
    review = os.path.join(root, "calc.diffnote")
    out = diffnote("edit", "-f", review, "--author", "reviewer", "c1..c2", cwd=repo, comments=[
        ("GLOBAL", "全体として、テストが追加されていないのが気になります。"),
        ("+        return None", "None を返すと呼び出し側が気づけません。"),
        ("@raw:+        return None", ">!resolve"),
        ("+    return a * b", "mul の型を確認してください。"),
    ])
    assert out.returncode == 0, out.stdout + out.stderr
    out = diffnote("edit", "-f", review, "--author", "reviewer", "c1..c3", cwd=repo, comments=[
        ("+\"\"\"calc\"\"\"", "docstring は 1 行でよいです。"),
    ])
    assert out.returncode == 0, out.stdout + out.stderr
    return review, repo


def write(dirname, name, text):
    path = os.path.join(dirname, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


LOGIN_V1 = ("import { Router } from 'express'\n\nconst router = Router()\n\n"
            "router.post('/login', async (req, res) => {\n  const { user, pass } = req.body\n"
            "  const account = await db.find(user)\n  if (account.pass === pass) {\n    res.json({ token: sign(account) })\n"
            "  } else {\n    res.status(401).end()\n  }\n})\n")
LOGIN_V2 = ("import { Router } from 'express'\nimport { compare } from './crypto'\n\nconst router = Router()\n\n"
            "router.post('/login', async (req, res) => {\n  const { user, pass } = req.body\n"
            "  const account = await db.find(user)\n  if (!account) {\n    return res.status(401).end()\n  }\n"
            "  const ok = await compare(pass, account.hash)\n  if (ok) {\n    res.json({ token: sign(account) })\n"
            "  } else {\n    res.status(401).end()\n  }\n})\n")


def make_login_review(root, snapshot=None, name="login"):
    """A git review with a diff of one file that has removed and added lines
    together, and untouched files (docs, a long file, a binary, a huge one)
    in the repository. `snapshot` is passed to `edit --snapshot`."""
    repo = os.path.join(root, name)
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    write(repo, "src/auth/login.ts", LOGIN_V1)
    write(repo, "src/util/b.ts", "export const x = 1\n")
    write(repo, "docs/README.md", "# 概要\n\nこのプロジェクトについて。\n")
    write(repo, "docs/設計 メモ.md", "メモ\n")
    write(repo, "big.txt", "".join(f"line {n}\n" for n in range(1, 1201)))
    with open(os.path.join(repo, "data.bin"), "wb") as f:
        f.write(b"\xff\xfe\x00\x9f")
    write(repo, "huge.txt", "x\n" * 1_500_000)
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "c1")
    git(repo, "tag", "c1")
    write(repo, "src/auth/login.ts", LOGIN_V2)
    git(repo, "commit", "-q", "-am", "c2")
    git(repo, "tag", "c2")
    review = os.path.join(root, name + ".diffnote")
    extra = ["--snapshot", snapshot] if snapshot else []
    out = diffnote("edit", "-f", review, "--author", "reviewer", *extra, "c1..c2", cwd=repo, comments=[
        ("+import { compare } from './crypto'", "`hash` は使っていません。"),
    ])
    assert out.returncode == 0, out.stdout + out.stderr
    return review, repo


class BrowserCase(unittest.TestCase):
    """A test case with a temporary directory and a headless Chrome."""

    @classmethod
    def setUpClass(cls):
        require_environment()
        cls.root = tempfile.mkdtemp(prefix="dn-browser-")
        cls.browser = Browser()

    @classmethod
    def tearDownClass(cls):
        cls.browser.close()
        shutil.rmtree(cls.root, ignore_errors=True)

    def fresh(self, name):
        """A new directory for one test's reviews."""
        path = tempfile.mkdtemp(prefix=name + "-", dir=self.root)
        return path
