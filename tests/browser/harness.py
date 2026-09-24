"""Helpers for the browser tests: a small Chrome DevTools client (standard
library only), a headless Chrome, `diffnote review`, and reviews to open.

The reviews are made by the real `diffnote`: `review` records the revisions
and its own API writes the comments, the way a person makes one. So the
tests need only Python, Chrome and a built `diffnote` (set DIFFNOTE_BIN,
or `cargo build` for target/debug).
"""
import base64
import http.client
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import urllib.parse
import sys
import tempfile
import time
import unittest
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
EXE = "diffnote.exe" if os.name == "nt" else "diffnote"
BIN = os.environ.get("DIFFNOTE_BIN") or os.path.join(ROOT, "target", "debug", EXE)
# Isolated from whatever `diffnote config` this machine actually has.
USER_CONFIG_DIR = tempfile.mkdtemp(prefix="dn-user-config-")


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
        by value: wrap it in `!!`.)

        A wait that runs out says what it saw instead, and what it sees a
        second later -- which is what tells a page that was only slow from one
        that was never going to get there.
        """
        end = time.time() + timeout
        seen = None
        first = True
        while time.time() < end:
            try:
                seen = self.js(expression)
            except RuntimeError:
                # A page in the middle of drawing itself: what a condition
                # reads can be missing for a moment. Only the first look
                # throwing is the expression's own fault.
                if first:
                    raise
                seen = None
            first = False
            if seen:
                return True
            time.sleep(0.03)
        try:
            later = repr(self.js(expression))
        except RuntimeError as e:
            later = "threw %s" % e
        print("\n[wait: %ss was not enough] %s\n  saw: %r\n  a second later: %s"
              % (timeout, expression, seen, later), file=sys.stderr)
        return False

    def mouse(self, kind, x, y, buttons=0, modifiers=0):
        self.call("Input.dispatchMouseEvent", type=kind, x=x, y=y, button="left",
                  buttons=buttons, clickCount=1, modifiers=modifiers)

    def key(self, key, code, vk):
        self.call("Input.dispatchKeyEvent", type="keyDown", key=key, code=code, windowsVirtualKeyCode=vk)


# How Chrome is started. Beyond the headless basics, these are all about a
# page that nobody is looking at: Chrome would otherwise treat one as
# backgrounded and slow its timers down -- or stop them -- which is exactly
# what a test that waits for something to happen cannot have. (Tests have been
# seen waiting for something the page was never going to get round to.)
CHROME_FLAGS = [
    "--headless=new",
    "--no-sandbox",
    "--disable-gpu",
    # Shared memory is small in some containers; Chrome falls over without it.
    "--disable-dev-shm-usage",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    # A page that changes the address often (which these do) can be throttled.
    "--disable-ipc-flooding-protection",
    # Nothing to set up: the profile is thrown away anyway.
    "--no-first-run",
    "--disable-extensions",
]
# (`--no-default-browser-check` is deliberately not here: with it, a drag over
# the diff stops selecting anything, and the test that copies what was dragged
# over fails every time. Whatever it does, it is not worth finding out for a
# profile that lives for one test class.)


class Browser:
    """A headless Chrome with one page, and helpers to look at it."""

    def __init__(self, attempts=3):
        # A Chrome that is slow to come up (a busy CI machine) is waited on for
        # its port; one that never says it is started again, not read half-way.
        for attempt in range(attempts):
            self.profile = tempfile.mkdtemp(prefix="dn-chrome-")
            self.proc = subprocess.Popen(
                [find_chrome(), *CHROME_FLAGS, "--remote-debugging-port=0",
                 f"--user-data-dir={self.profile}", "--window-size=1500,900", "about:blank"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            port = self._port(os.path.join(self.profile, "DevToolsActivePort"))
            if port:
                break
            self._stop()
        else:
            raise RuntimeError(f"Chrome did not start ({attempts} tries)")
        self.cdp = Cdp(port)
        self.cdp.call("Page.enable")

    def _port(self, port_file, timeout=30):
        """The port Chrome says it listens on, once it has written all of it
        (the file can be there, and empty, a moment before); `None` if it
        doesn't within `timeout`, or stops."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc.poll() is not None:
                return None
            try:
                with open(port_file) as f:
                    line = f.readline()
                if line.endswith("\n") and line.strip().isdigit():
                    return int(line)
            except OSError:
                pass
            time.sleep(0.05)
        return None

    def _stop(self):
        try:
            self.proc.kill()
            self.proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            pass
        shutil.rmtree(self.profile, ignore_errors=True)

    def close(self):
        """Stop it, and be sure it is stopped.

        A Chrome that doesn't go on being asked (it does happen) used to be
        waited on until the wait gave up, which left the whole of it running --
        and its profile on disk -- for the rest of the suite. A test class each
        starts one, so they pile up, and every page after them has less machine
        to draw on: the waits that were failing about one full run in four were
        this.
        """
        try:
            self.cdp.sock.close()
        except OSError:
            pass
        try:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            pass
        shutil.rmtree(self.profile, ignore_errors=True)

    # -- page --
    # The page that was there answers `ready` too until the new one takes
    # its place: it is marked first, and waited on to be gone.
    GONE = "!window.__dnLeaving"

    def open(self, url, ready="!!document.querySelector('.diffnote-diff')"):
        self._leave()
        went = self.cdp.call("Page.navigate", url=url).get("result", {})
        if "loaderId" not in went:
            # Only the `#...` moved: the same page stays (and is the new one).
            self.cdp.js("window.__dnLeaving = false")
        assert self.cdp.wait(f"{self.GONE} && document.readyState==='complete' && {ready}"), f"{url} did not load"

    def reload(self, ready="!!document.querySelector('.diffnote-diff')"):
        self._leave()
        self.cdp.call("Page.reload")
        assert self.cdp.wait(f"{self.GONE} && document.readyState==='complete' && {ready}")

    def _leave(self):
        try:
            self.cdp.js("window.__dnLeaving = true")
        except RuntimeError:
            pass

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
        # As typing does: the page hears an `input` (and gets a moment to draw).
        self.js("var t=document.querySelector(%s); Object.getOwnPropertyDescriptor(Object.getPrototypeOf(t),'value').set.call(t,%s); t.dispatchEvent(new Event('input',{bubbles:true}))"
                % (json.dumps(selector), json.dumps(text)))
        time.sleep(0.1)

    def click(self, selector):
        self.js("document.querySelector(%s).click()" % json.dumps(selector))

    def center(self, selector):
        return self.js("(function(){var e=document.querySelector(%s); e.scrollIntoView({block:'center'}); var r=e.getBoundingClientRect(); return [r.x+r.width/2, r.y+r.height/2]})()" % json.dumps(selector))

    def start_of(self, selector):
        """Just inside the left edge of an element, level with its middle.

        Where a press falls inside a line of text decides where a selection
        starts, and the middle of a cell is a different place in a different
        column width or font. This is the beginning of the line wherever it
        is drawn.
        """
        return self.js("(function(){var e=document.querySelector(%s); e.scrollIntoView({block:'center'});"
                       " var r=e.getBoundingClientRect(); return [r.x+2, r.y+r.height/2]})()" % json.dumps(selector))

    def press(self, selector, modifiers=0):
        x, y = self.center(selector)
        self.cdp.mouse("mouseMoved", x, y)
        self.cdp.mouse("mousePressed", x, y, 1, modifiers)
        return x, y

    def release(self, selector=None, modifiers=0):
        x, y = self.center(selector) if selector else (0, 0)
        self.cdp.mouse("mouseReleased", x, y, 0, modifiers)

    def click_at(self, selector, modifiers=0):
        """Presses and lets go at one point.

        Measured once: pressing can move what was pressed (choosing a line
        opens a box under it, which pushes the rest of the table down), and
        letting go at a freshly measured point would be letting go somewhere
        else -- which is a drag, not a click.
        """
        x, y = self.center(selector)
        self.cdp.mouse("mouseMoved", x, y)
        self.cdp.mouse("mousePressed", x, y, 1, modifiers)
        self.cdp.mouse("mouseReleased", x, y, 0, modifiers)

    def drag(self, first, last, steps=8, from_start=False):
        """Presses on `first`, moves to `last` with the button held, and lets
        go there.

        The move is made in steps, as a hand makes it. One jump from the
        press to the release leaves some builds of Chrome with no selection
        at all (the browser tests' drag over a diff has never selected
        anything on the CI runner, while passing here): what a drag is, to
        a browser, is a press followed by movement.

        `from_start` presses at the beginning of `first` rather than in the
        middle of it, for a drag that is meant to take whole lines.
        """
        if from_start:
            x, y = self.start_of(first)
            self.cdp.mouse("mouseMoved", x, y)
            self.cdp.mouse("mousePressed", x, y, 1)
        else:
            x, y = self.press(first)
        x2, y2 = self.center(last)
        # A few pixels first: a browser starts selecting once the pointer has
        # moved past its own threshold, and a first step of an eighth of the
        # way is a jump, not a movement.
        for nudge in (2, 5, 9):
            self.cdp.mouse("mouseMoved", x + nudge, y, 1)
        for i in range(1, steps + 1):
            self.cdp.mouse("mouseMoved", round(x + (x2 - x) * i / steps),
                           round(y + (y2 - y) * i / steps), 1)
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

def set_user_author(name):
    """Sets the author name `review`/`open` fall back to when none is given
    (the replacement for the removed `--author` flag: `diffnote config`,
    isolated to USER_CONFIG_DIR like every diffnote() call here)."""
    out = diffnote("config", "set", "author", name)
    assert out.returncode == 0, out.stdout + out.stderr


def set_user_view(view):
    """What the user settings say of how the page is shown (`None`: nothing,
    so a served page starts from what the browser keeps, then its defaults).
    Every served page reads it, so each `Served` starts from this."""
    path = os.path.join(USER_CONFIG_DIR, "config.json")
    try:
        with open(path, encoding="utf-8") as f:
            config = json.load(f)
    except (OSError, ValueError):
        config = {}
    config.pop("view", None)
    if view:
        config["view"] = view
    os.makedirs(USER_CONFIG_DIR, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(config, f)


def user_view():
    """How the page is shown, as the user settings keep it."""
    try:
        with open(os.path.join(USER_CONFIG_DIR, "config.json"), encoding="utf-8") as f:
            return json.load(f).get("view", {})
    except (OSError, ValueError):
        return {}


def diffnote(*args, cwd=None, env=None):
    """Runs `diffnote` and gives back what it did. Comments are not written
    this way any more: a review is made through the page's own API, which is
    how a person makes one (see `review_of`)."""
    full_env = dict(os.environ)
    full_env["DIFFNOTE_CONFIG_DIR"] = USER_CONFIG_DIR
    if env:
        full_env.update(env)
    return subprocess.run([BIN, *args], cwd=cwd, env=full_env, capture_output=True, text=True, encoding="utf-8")


def member(review, name):
    """One file out of the bundle, as text (empty when it has none)."""
    out = subprocess.run(["unzip", "-p", review, name], capture_output=True, text=True, encoding="utf-8")
    return out.stdout if out.returncode == 0 else ""


def log(review):
    """The review's events, parsed, in the order they were recorded."""
    return [json.loads(l) for l in member(review, "review.jsonl").splitlines() if l.strip()]


def _side(side):
    """One side of an anchor as `path:line` or `path:first-last` (a range of
    no lines is the point where they were, and is named by that line)."""
    start, length = side.get("start", 0), side.get("len", 0)
    at = str(start) if length <= 1 else "%d-%d" % (start, start + length - 1)
    return "%s:%s" % (side.get("file", ""), at)


def _where(anchor):
    """Where a comment is anchored: the head side, then the base side it came
    from. `GLOBAL` for the review, `ファイル全体: path` for a whole file."""
    if not anchor:
        return ""
    scope = anchor.get("scope")
    if scope == "global":
        return "GLOBAL"
    if scope == "file":
        side = anchor.get("head") or anchor.get("base") or {}
        return "ファイル全体: " + side.get("file", "")
    head, base = anchor.get("head"), anchor.get("base")
    # The base side is named only when it has lines of its own: a comment on
    # an added line has a base side too, but it is the point the line was
    # put at, which says nothing about where the comment is.
    sides = []
    if head:
        sides.append(_side(head))
    if base and (not head or base.get("len", 0) > 0):
        sides.append(_side(base))
    return " <- ".join(sides)


def recorded(review):
    """Everything the bundle records, as lines to search.

    The tests ask "is this recorded?", and this answers it from the bundle
    itself: the event log and the settings, rather than the output of a
    command. One line per thing, so a test can look at a line at a time.
    """
    out = []
    settings = member(review, "settings.json")
    if settings:
        for key, value in sorted(json.loads(settings).items()):
            out.append("設定 %s=%s" % (key, value))
    for e in log(review):
        kind = e["kind"]
        if kind == "comment":
            out.append("コメント %s %s %s | %s" % (
                e["id"], e["author"],
                "返信" if e.get("parent") else "新規 " + _where(e.get("anchor")),
                e["body"].replace("\n", " ")))
        elif kind in ("resolve", "reopen"):
            out.append("%s %s %s" % ("解決" if kind == "resolve" else "再開", e["parent"], e["author"]))
        elif kind == "reanchor":
            out.append("付け替え %s %s" % (e["parent"], _where(e.get("anchor"))))
        elif kind == "pin":
            out.append("固定 %s %d 個" % (e["revision"], len(e.get("files", []))))
        elif kind == "revision":
            out.append("リビジョン %s %s" % (e["id"], e["digest"]))
        else:
            out.append(kind)
    return "\n".join(out) + "\n"


def entries(review):
    """The number of events in the review's log."""
    return len(log(review))


def zip_names(review):
    return subprocess.run(["unzip", "-Z1", review], capture_output=True, text=True).stdout.split()


class Served:
    """`diffnote review` on a review (`command="open"`: `diffnote open`, the
    review as it is)."""

    def __init__(self, review, cwd=None, extra=(), author="tester", command="review", view=None, keep_view=False):
        # The server has no --author of its own any more (diffnote config does
        # its job): author=None starts it with whatever is already configured
        # (or, with nothing configured, git config then the login name);
        # otherwise it is set into the (isolated) user config first, for this
        # process to pick up at startup -- and then taken out again (a `Served`
        # this test didn't ask for shouldn't leave its name configured for
        # whatever runs next; the session itself already has it, resolved once
        # at startup, regardless of the file).
        self.review = review
        config_file = os.path.join(USER_CONFIG_DIR, "config.json")
        if author is not None:
            set_user_author(author)
        # What one test chose on its page is not the next one's to start with
        # (unless the test is about just that: `keep_view`).
        if not keep_view:
            set_user_view(view)
        env = dict(os.environ)
        env["DIFFNOTE_CONFIG_DIR"] = USER_CONFIG_DIR
        self.proc = subprocess.Popen([BIN, command, "-f", review, "--no-browser", *extra],
                                     cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8")
        self.notices = []
        self.said = []
        self.url = None
        self._token_cookie = None
        for line in self.proc.stdout:
            self.said.append(line.strip())
            if "注意" in line:
                self.notices.append(line.strip())
            m = re.search(r"(http://127\.0\.0\.1:\d+/\?t=\w+)", line)
            if m:
                self.url = m.group(1)
                break
        assert self.url, "diffnote did not start"
        if author is not None and os.path.exists(config_file):
            os.remove(config_file)

    def _connect(self):
        parts = urllib.parse.urlsplit(self.url)
        return http.client.HTTPConnection(parts.hostname, parts.port, timeout=15)

    def _cookie(self):
        """The token, kept the way the page keeps it: the first visit carries
        it in the address and is answered with a cookie."""
        if self._token_cookie is None:
            parts = urllib.parse.urlsplit(self.url)
            c = self._connect()
            c.request("GET", parts.path + "?" + parts.query)
            answer = c.getresponse()
            answer.read()
            got = answer.getheader("Set-Cookie")
            assert got, "the server gave no cookie for %s" % self.url
            self._token_cookie = got.split(";")[0]
            c.close()
        return self._token_cookie

    def api(self, path, data=None):
        """A request to the review, as the page makes them: JSON in, JSON
        out, with the token and the header the server insists on."""
        c = self._connect()
        body = None if data is None else json.dumps(data).encode("utf-8")
        headers = {"X-Diffnote": "1", "Cookie": self._cookie()}
        if body is not None:
            headers["Content-Type"] = "application/json"
        c.request("GET" if body is None else "POST", path, body, headers)
        answer = c.getresponse()
        text = answer.read().decode("utf-8")
        c.close()
        assert answer.status == 200, "%s: %d %s" % (path, answer.status, text[:300])
        return json.loads(text) if text.strip() else {}

    def said_more(self):
        """What the server has said since it started: waits for it to stop, then reads the rest."""
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        rest = self.proc.stdout.read() if self.proc.poll() is not None else ""
        self.said.extend(l.strip() for l in rest.splitlines())
        return self.said

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

def line_of(repo, rev, path, text):
    """Which line of `path` at `rev` reads exactly `text`.

    Fixtures name a line by what it says, not by counting: a line that moves
    when the file above it changes should not move a test with it.
    """
    out = subprocess.run(["git", "-C", repo, "show", "%s:%s" % (rev, path)],
                         capture_output=True, text=True, encoding="utf-8")
    assert out.returncode == 0, "git show %s:%s: %s" % (rev, path, out.stderr)
    for n, line in enumerate(out.stdout.split("\n"), 1):
        if line == text:
            return n
    raise AssertionError("%s:%s has no line %r" % (rev, path, text))


def write_comments(server, repo, rev, revision, comments):
    """Writes `comments` into a served review through its own API -- the way
    a person writes them, without an editor in the middle.

    Each is a dict with a `body` and where it goes: `global`, `file` for a
    whole file, `file` with `line` (or `lines`, a first and last) for lines,
    or `reply` (the index of an earlier one in this list). `resolve` marks
    the thread resolved once it is written. Gives back the thread ids, in
    order, so a later comment can name one.
    """
    ids = []
    for c in comments:
        if "reply" in c:
            thread = ids[c["reply"]]
            server.api("/api/threads/%s/replies" % thread, {"body": c["body"]})
            ids.append(thread)
        else:
            ask = {"body": c["body"], "revision": revision}
            if c.get("global"):
                ask["scope"] = "global"
            elif "line" in c or "lines" in c:
                first, last = c["lines"] if "lines" in c else (c["line"], c["line"])
                start = line_of(repo, rev, c["file"], first)
                end = line_of(repo, rev, c["file"], last)
                ask.update(scope="lines", file=c["file"],
                           head={"start": start, "len": end - start + 1})
            else:
                ask.update(scope="file", file=c["file"])
            ids.append(server.api("/api/threads", ask)["thread"])
        if c.get("resolve"):
            server.api("/api/threads/%s/resolve" % ids[-1], {})
    return ids


def review_of(repo, review, target, comments=(), base=None, author="reviewer", extra=()):
    """Adds `target` to `review` (making it, with `base`, if it is not there
    yet) and writes `comments` into it, through `review`."""
    args = (["--base", base] if base else []) + [target, *extra]
    server = Served(review, cwd=repo, author=author, extra=args)
    try:
        revision = len(server.api("/api/model")["model"]["revisions"]) - 1
        write_comments(server, repo, target, revision, comments)
    finally:
        server.stop()



def git(repo, *args):
    out = subprocess.run(["git", "-C", repo, "-c", "user.email=t@example.com", "-c", "user.name=T", *args],
                         capture_output=True, text=True, encoding="utf-8")
    assert out.returncode == 0, f"git {args}: {out.stderr}"
    return out.stdout.strip()


CALC_V1 = "def add(a, b):\n    return a + b\n\n\ndef div(a, b):\n    return a / b\n"
CALC_V2 = ("def add(a, b):\n    return a + b\n\n\ndef div(a, b):\n    if b == 0:\n        return None\n"
           "    return a / b\n\n\ndef mul(a, b):\n    return a * b\n")
CALC_V3 = '"""calc"""\n\n' + CALC_V2.replace("return None", 'raise ValueError("b is zero")')


def make_gaps_review(root, name="gaps"):
    """A git review of a 100-line file changed at lines 20 and 80 (and a comment
    on each): the diff leaves out lines 1-16, 24-76 and 84-100, of which the
    middle place is the longest."""
    repo = os.path.join(root, name)
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    text = lambda a, b: "".join({20: a + "\n", 80: b + "\n"}.get(n, f"row {n}\n") for n in range(1, 101))
    write(repo, "long.txt", text("twenty", "eighty"))
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "c1")
    git(repo, "tag", "c1")
    write(repo, "long.txt", text("TWENTY", "EIGHTY"))
    git(repo, "commit", "-q", "-am", "c2")
    git(repo, "tag", "c2")
    review = os.path.join(root, name + ".diffnote")
    review_of(repo, review, "c2", base="c1", comments=[
        {"file": "long.txt", "line": "TWENTY", "body": "20 行目を変えました。"},
    ])
    return review, repo


def add_settings(review, settings):
    """Puts a settings.json (the review's settings as state) into a bundle."""
    import zipfile
    with zipfile.ZipFile(review, "a", zipfile.ZIP_DEFLATED) as z:
        z.writestr("settings.json", json.dumps(settings))


def make_indent_review(root):
    """A git review of a file whose lines a and b were only re-indented, and c changed."""
    repo = os.path.join(root, "indent")
    os.makedirs(repo)
    git(repo, "init", "-q", "-b", "main")
    write(repo, "x.py", "def f():\n  a = 1\n  b = 2\n  c = 3\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "c1")
    git(repo, "tag", "c1")
    write(repo, "x.py", "def f():\n    a = 1\n    b = 2\n    c = 4\n")
    git(repo, "commit", "-q", "-am", "c2")
    git(repo, "tag", "c2")
    review = os.path.join(root, "indent.diffnote")
    review_of(repo, review, "c2", base="c1", comments=[
        {"file": "x.py", "line": "    c = 4", "body": "c を変えました。"},
    ])
    return review, repo


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
    review_of(repo, review, "c2", base="c1", comments=[
        {"global": True, "body": "全体として、テストが追加されていないのが気になります。"},
        {"file": "calc.py", "line": "        return None", "resolve": True,
         "body": "None を返すと呼び出し側が気づけません。"},
        {"file": "calc.py", "line": "    return a * b", "body": "mul の型を確認してください。"},
    ])
    review_of(repo, review, "c3", comments=[
        {"file": "calc.py", "line": '"""calc"""', "body": "docstring は 1 行でよいです。"},
    ])
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
    in the repository. `snapshot` is what the review keeps (`--snapshot`)."""
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
    review_of(repo, review, "c2", base="c1", extra=extra, comments=[
        {"file": "src/auth/login.ts", "line": "import { compare } from './crypto'",
         "body": "`hash` は使っていません。"},
    ])
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
