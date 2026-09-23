"""The exported page (opened from a file, no server)."""
import json
import os
import shutil
import pathlib
import time
import sys
import unittest

import harness
import harness
from harness import BrowserCase, add_settings, diffnote, git, make_calc_review, make_indent_review, make_gaps_review, make_login_review, write

CUR = ".diffnote-revision.is-current"


class StaticExport(BrowserCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        review, _ = make_calc_review(cls.root)
        html = os.path.join(cls.root, "out.html")
        out = diffnote("export", "-f", review, html)
        assert out.returncode == 0, out.stderr
        cls.url = pathlib.Path(html).as_uri()

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url)
        # Every test starts with nothing kept by the browser but the unified
        # layout (a first visit in a wide window starts side by side).
        self.b.js("localStorage.clear(); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()

    def card(self, text):
        """Mark the visible thread card that says `text`, so the mouse can find it."""
        self.b.js("""(function(){
          document.querySelectorAll('[data-test-card]').forEach(function(e){e.removeAttribute('data-test-card')});
          var c = Array.from(document.querySelectorAll('%s .diffnote-thread')).find(function(t){return t.textContent.includes(%s)});
          c.setAttribute('data-test-card','1'); c.scrollIntoView({block:'center'});
        })()""" % (CUR, repr(text)))
        return "[data-test-card] summary"

    def test_a_file_marked_as_looked_at_is_hidden_with_its_threads_and_can_be_brought_back(self):
        b = self.b
        files = b.count(f"{CUR} section.diffnote-file")
        self.assertEqual(b.js("document.querySelector('[data-diffnote-viewed-count]').textContent.trim()"), f"0/{files}")
        self.assertTrue(b.exists("[data-diffnote-viewed-count] svg.diffnote-icon"), "with a check drawn, not a character")
        threads_before = b.count(".diffnote-threadlist li")
        section = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py']"
        self.assertTrue(b.exists(section))
        gap = b.js(f"(() => {{ const s = document.querySelector({json.dumps(section + ' summary')}).getBoundingClientRect(); const b = document.querySelector({json.dumps(section + ' [data-diffnote-viewed]')}).getBoundingClientRect(); return s.right - b.right; }})()")
        self.assertLess(gap, 16, "the button is at the right end of the header")
        b.click(f"{section} [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)})"), "the file and its threads are gone")
        self.assertEqual(b.count(f"{CUR} section.diffnote-file"), files - 1)
        self.assertIn(f"1/{files}", b.js("document.querySelector('[data-diffnote-viewed-count]').textContent"))
        item = "[data-diffnote-check='calc.py']"
        self.assertTrue(b.exists(item + " svg.diffnote-icon"), "the file is marked with a check")
        self.assertTrue(b.exists("li.is-viewed [data-diffnote-open-count]"), "what is still open is said, small")
        self.assertEqual(b.count(".diffnote-threadlist li"), threads_before, "its threads are still listed")
        b.click(item)
        self.assertTrue(b.wait(f"!!document.querySelector({json.dumps(section)})"), "brought back")
        self.assertEqual(b.count(".diffnote-threadlist li"), threads_before)
        self.assertIn(f"0/{files}", b.js("document.querySelector('[data-diffnote-viewed-count]').textContent"))

    def test_a_thread_picked_in_the_list_brings_back_the_file_it_is_on(self):
        b = self.b
        section = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py']"
        b.click(f"{section} [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)})"))
        b.click(".diffnote-threadlist a[title^='calc.py']")
        self.assertTrue(b.wait(f"!!document.querySelector({json.dumps(section)})"), "the file is back")
        self.assertFalse(b.exists("[data-diffnote-check='calc.py'][aria-pressed='true']"))
        # It goes to the thread as well: the card is on the screen.
        self.assertTrue(b.wait(f"(() => {{ const c = document.querySelector({json.dumps(section + ' .diffnote-thread')}); if (!c) return false; const r = c.getBoundingClientRect(); return r.bottom > 0 && r.top < innerHeight; }})()"))

    def test_a_looked_at_file_that_is_another_file_in_the_other_revision_is_not_marked_there(self):
        b = self.b
        b.click(f"{CUR} section.diffnote-file[data-diffnote-file='calc.py'] [data-diffnote-viewed]")
        self.assertTrue(b.wait("!document.querySelector('section.diffnote-file[data-diffnote-file=\"calc.py\"]')"))
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait_exists("section.diffnote-file[data-diffnote-file='calc.py']"), "calc.py differs in #1")

    def test_the_title_of_an_export_is_only_a_heading(self):
        b = self.b
        self.assertFalse(b.exists("[data-diffnote-screen-open]"), "nothing to set in a page that only shows the review")
        self.assertIn("diffnote レビュー", b.text(".diffnote-summary h1"))

    def test_the_view_menu_is_at_the_top_right_of_the_diff_and_shut_until_opened(self):
        b = self.b
        panel = "[data-diffnote-view-panel]"
        self.assertTrue(b.js(f"document.querySelector({json.dumps(panel)}).hidden"), "shut")
        self.assertFalse(b.js("!!document.querySelector('.diffnote-topbar [data-diffnote-ignore-space], .diffnote-topbar [data-diffnote-hide-resolved]')"),
                         "not in the top bar")
        # At the right end of the diff's column, above the files.
        pos = b.js("(() => { const m = document.querySelector('[data-diffnote-view-menu]').getBoundingClientRect(); const f = document.querySelector('section.diffnote-file').getBoundingClientRect(); const s = document.querySelector('.diffnote-sidebar').getBoundingClientRect(); return {right: f.right - m.right, above: m.bottom <= f.top, beside: m.left > s.right}; })()")
        self.assertLess(pos["right"], 20)
        self.assertTrue(pos["above"] and pos["beside"], pos)
        b.click("[data-diffnote-view-menu]")
        self.assertFalse(b.js(f"document.querySelector({json.dumps(panel)}).hidden"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-layout=unified]').classList.contains('is-current')"), True)
        time.sleep(0.2)  # (the menu listens for Escape once it has been drawn)
        b.escape()
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(panel)}).hidden"), "Escape shuts it")

    def test_the_icons_are_drawn_in_the_page_and_take_the_colour_of_their_text(self):
        b = self.b
        self.assertGreater(b.count("svg.diffnote-icon"), 5, "the page's own marks are shapes")
        self.assertTrue(b.js("""[...document.querySelectorAll('svg.diffnote-icon')].every(function (s) {
          return s.children.length === 1 && s.firstElementChild.tagName === 'path' && !s.textContent;
        })"""), "one shape each, with no text of its own")
        # Drawn here: an exported page is opened from a file and has no font
        # or picture to go and get.
        self.assertEqual(b.count("img, link, iframe, object"), 0)
        got = b.js("""(() => {
          const button = document.querySelector('[data-diffnote-view-menu]');
          const icon = button.querySelector('svg.diffnote-icon');
          const box = icon.getBoundingClientRect();
          return [getComputedStyle(icon).fill, getComputedStyle(button).color, box.width, parseFloat(getComputedStyle(button).fontSize)];
        })()""")
        self.assertEqual(got[0], got[1], "the icon is the colour of the words beside it")
        self.assertGreater(got[2], got[3], "and a little bigger than them")
        self.assertLess(got[2], got[3] * 1.5)

    def test_the_lists_beside_the_diff_can_be_put_away_and_brought_back(self):
        b = self.b
        toggle = "[data-diffnote-sidebar-toggle]"
        lists = ".diffnote-sidebar__lists"
        width = lambda sel: b.js("document.querySelector(%s).getBoundingClientRect().width" % json.dumps(CUR + " " + sel))
        self.assertTrue(b.visible(lists))
        diff, column = width("section.diffnote-file"), width(".diffnote-sidebar")
        # The button is at the foot of the lists' own column, by its right edge.
        foot = b.js("(() => { const f = document.querySelector('%s .diffnote-sidebar__foot').getBoundingClientRect();"
                    " const t = document.querySelector('%s %s').getBoundingClientRect();"
                    " const s = document.querySelector('%s .diffnote-sidebar').getBoundingClientRect();"
                    " return {bottom: s.bottom - f.bottom, right: f.right - t.right, inside: t.left >= s.left}; })()"
                    % (CUR, CUR, toggle, CUR))
        self.assertLess(foot["bottom"], 12, foot)
        self.assertLess(foot["right"], 4, foot)
        self.assertTrue(foot["inside"], foot)

        b.click(toggle)
        self.assertTrue(b.wait("!document.querySelector('%s:not([hidden])')" % lists))
        self.assertGreater(width("section.diffnote-file"), diff, "the diff takes the room the lists had")
        # All that is left of the column is the button, which is how they come back.
        self.assertTrue(b.visible(toggle))
        self.assertLess(width(".diffnote-sidebar"), width(toggle) + 16, "no wider than the button")
        self.assertEqual(b.js("document.querySelector('%s %s').getAttribute('aria-expanded')" % (CUR, toggle)), "false")
        b.click(toggle)
        self.assertTrue(b.wait("!!document.querySelector('%s:not([hidden])')" % lists))
        self.assertEqual(width(".diffnote-sidebar"), column, "and the column comes back as it was")
        # It is for making room for a moment: opening the page again has them.
        b.click(toggle)
        self.assertTrue(b.wait("!document.querySelector('%s:not([hidden])')" % lists))
        b.reload()
        self.assertTrue(b.visible(lists), "not kept, unlike the layout")

    def test_the_latest_revision_is_shown_and_the_tabs_switch(self):
        b = self.b
        self.assertEqual(b.count("[data-diffnote-revision-link]"), 2)
        self.assertEqual(b.js("document.querySelector('.diffnote-revision.is-current').id"), "rev-1")
        self.assertEqual(b.count(".diffnote-revision"), 1, "only the one being looked at is drawn")
        b.click("[data-diffnote-revision-link='0']")
        self.assertEqual(b.js("document.querySelector('.diffnote-revision.is-current').id"), "rev-0")
        self.assertEqual(b.count(".diffnote-revision"), 1)
        self.assertTrue(b.js("document.querySelector('[data-diffnote-revision-link=\"0\"]').classList.contains('is-current')"))

    def test_hovering_a_thread_shows_its_whole_range_and_leaving_hides_it(self):
        b = self.b
        b.hover(self.card("mul の型"))
        self.assertGreaterEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 1)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-range-first').length"), 1)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-range-last').length"), 1)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-thread.diffnote-hover').length"), 1)
        b.cdp.mouse("mouseMoved", 5, 5)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 0)

    def test_code_is_colored_by_the_kind_of_each_piece(self):
        b = self.b
        kinds = b.js("Array.from(new Set(Array.from(document.querySelectorAll('.diffnote-diff .tok')).map(function(e){return e.className}))).sort()")
        self.assertIn("tok tok-keyword", kinds)
        plain = b.js("getComputedStyle(document.querySelector('.diffnote-diff code')).color")
        keyword = b.js("getComputedStyle(document.querySelector('.diffnote-diff .tok-keyword')).color")
        self.assertNotEqual(plain, keyword)

    def test_a_click_pins_the_range_and_escape_lets_go(self):
        b = self.b
        sel = self.card("mul の型")
        b.click_at(sel)
        b.cdp.mouse("mouseMoved", 5, 5)
        self.assertGreaterEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 1, "pinned")
        b.escape()
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 0)

    def test_the_copy_buttons_copy_the_path_and_the_location(self):
        b = self.b
        b.stub_clipboard()
        was_open = b.js(f"document.querySelector('{CUR} section.diffnote-file details').open")
        b.click(f"{CUR} section.diffnote-file summary .diffnote-copy")
        self.assertEqual(b.js("window.__copied"), "calc.py")
        self.assertEqual(b.js(f"document.querySelector('{CUR} section.diffnote-file details').open"), was_open,
                         "the button does not fold the file")
        sel = self.card("mul の型")
        where = b.js("document.querySelector('[data-test-card] .diffnote-thread__where').textContent")
        self.assertRegex(where, r"^calc\.py:\d+$")
        b.click("[data-test-card] summary .diffnote-copy")
        self.assertEqual(b.js("window.__copied"), where + "@2", "with the revision it is in")

    def test_a_thread_in_the_list_leads_to_its_card_and_shows_its_range(self):
        b = self.b
        items = b.visible(f"{CUR} .diffnote-threadlist a")
        self.assertEqual(items, 3, "the global thread, `mul` and the docstring (the resolved one is hidden)")
        b.js(f"""(function(){{var a=Array.from(document.querySelectorAll('{CUR} .diffnote-threadlist a')).find(function(a){{return a.textContent.includes('mul')}}); a.click()}})()""")
        self.assertGreaterEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 1)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-thread.diffnote-hover').length"), 1)

    def test_resolved_threads_are_hidden_at_first_and_the_choice_is_kept(self):
        b = self.b
        self.assertTrue(b.js("document.querySelector('[data-diffnote-hide-resolved]').checked"))
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread--resolved"), 0)
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread"), 3)
        self.assertEqual(b.visible(f"{CUR} .diffnote-threadlist li"), 3)
        self.assertEqual(b.js("document.querySelector('[data-diffnote-resolved-count]').textContent"), "(1)")
        b.click("[data-diffnote-hide-resolved]")
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread"), 4)
        self.assertEqual(b.visible(f"{CUR} .diffnote-threadlist li"), 4)
        b.reload()
        self.assertFalse(b.js("document.querySelector('[data-diffnote-hide-resolved]').checked"), "kept: shown")
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread--resolved"), 1)
        b.click("[data-diffnote-hide-resolved]")
        b.reload()
        self.assertTrue(b.js("document.querySelector('[data-diffnote-hide-resolved]').checked"), "kept: hidden")

    def test_the_file_badges_count_the_threads_that_are_shown(self):
        b = self.b
        badge = f"{CUR} .diffnote-filelist .diffnote-badge"
        total = lambda: b.js("Array.from(document.querySelectorAll(%s)).reduce(function(n,e){return n+Number(e.textContent)},0)" % json.dumps(badge))
        # Of the 4 threads, one is the whole review's (in no file) and one is resolved.
        self.assertEqual(total(), 2)
        b.click("[data-diffnote-hide-resolved]")
        self.assertEqual(total(), 3)

    def test_a_line_that_only_a_resolved_thread_is_about_loses_its_mark_while_hidden(self):
        b = self.b
        b.click("[data-diffnote-revision-link='0']")
        bar = lambda row: b.js(f"getComputedStyle(document.querySelector('#rev-0 tr[data-diffnote-threads=\"{row}\"] td')).boxShadow.indexOf('rgb(') >= 0")
        rows = b.js("Array.from(document.querySelectorAll('#rev-0 tr[data-diffnote-threads]')).map(function(r){return r.getAttribute('data-diffnote-threads')})")
        resolved_only = b.js("document.querySelectorAll('#rev-0 .diffnote-line--resolved-only').length")
        self.assertEqual(resolved_only, 1)
        self.assertEqual(sum(1 for r in rows if bar(r)), len(rows) - 1)
        b.click("[data-diffnote-hide-resolved]")
        self.assertEqual(b.js("document.querySelectorAll('#rev-0 .diffnote-line--resolved-only').length"), 0)
        self.assertEqual(sum(1 for r in rows if bar(r)), len(rows))

    def test_hovering_a_line_of_a_hidden_resolved_thread_shows_no_range(self):
        b = self.b
        b.click("[data-diffnote-revision-link='0']")
        line = "#rev-0 .diffnote-line--resolved-only .diffnote-line__gutter-new, #rev-0 .diffnote-line--resolved-only .diffnote-line__gutter-old"
        self.assertGreaterEqual(b.count("#rev-0 .diffnote-line--resolved-only"), 1)
        b.hover(line)
        self.assertEqual(b.count(".diffnote-range"), 0, "hidden: no highlight")
        b.cdp.mouse("mouseMoved", 5, 5)
        b.click("[data-diffnote-hide-resolved]")
        b.hover("#rev-0 tr[data-diffnote-threads] .diffnote-line__gutter-new, #rev-0 tr[data-diffnote-threads] .diffnote-line__gutter-old")
        self.assertGreaterEqual(b.count(".diffnote-range"), 1)

    def test_the_page_fits_a_narrow_screen(self):
        b = self.b
        b.cdp.call("Emulation.setDeviceMetricsOverride", width=500, height=800, deviceScaleFactor=1, mobile=False)
        try:
            b.reload()
            self.assertLessEqual(b.js("document.documentElement.scrollWidth"), b.js("document.documentElement.clientWidth"))
        finally:
            b.cdp.call("Emulation.clearDeviceMetricsOverride")


class SideBySide(BrowserCase):
    """The diff in two columns, and the switch between the two layouts."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        review, _ = make_login_review(cls.root, name="split")
        html = os.path.join(cls.root, "split.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        cls.url = pathlib.Path(html).as_uri()
        # A file with one very long line that changes, for wrapping.
        repo = os.path.join(cls.root, "long")
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        write(repo, "wide.txt", "short\n" + "word " * 100 + "one\nend\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c1")
        git(repo, "tag", "c1")
        write(repo, "wide.txt", "short\n" + "word " * 100 + "two\nend\n")
        git(repo, "commit", "-q", "-am", "c2")
        git(repo, "tag", "c2")
        long_review = os.path.join(cls.root, "long.diffnote")
        assert diffnote("edit", "-f", long_review, "--base", "c1", "c2", cwd=repo,
                        comments=[("+" + "word " * 100 + "two", "wide")]).returncode == 0
        long_html = os.path.join(cls.root, "long.html")
        assert diffnote("export", "-f", long_review, long_html).returncode == 0
        cls.long_url = pathlib.Path(long_html).as_uri()

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url)
        self.b.js("localStorage.clear(); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()

    def split(self):
        self.b.click("[data-diffnote-layout='split']")
        self.assertTrue(self.b.wait_exists("table.diffnote-diff--split"))

    def test_the_switch_offers_two_layouts(self):
        b = self.b
        self.assertEqual(b.count(".diffnote-layout__button"), 2)
        self.assertEqual(b.text(".diffnote-layout__button.is-current"), "統合")
        self.assertEqual(b.count("table.diffnote-diff--split"), 0)
        self.assertGreater(b.count("table.diffnote-diff tr[class*='diffnote-line--']"), 0)

    def test_a_first_visit_starts_side_by_side_in_a_wide_window_and_unified_in_a_narrow_one(self):
        b = self.b
        b.js("localStorage.clear()")
        b.reload()
        self.assertEqual(b.text(".diffnote-layout__button.is-current"), "横並び", "the window is 1500 wide")
        self.assertEqual(b.count("table.diffnote-diff--split") > 0, True)
        self.assertEqual(b.js("localStorage.getItem('diffnote-layout')"), None, "not kept until chosen")
        b.cdp.call("Emulation.setDeviceMetricsOverride", width=1000, height=800, deviceScaleFactor=1, mobile=False)
        try:
            b.reload()
            self.assertEqual(b.text(".diffnote-layout__button.is-current"), "統合")
            # Only when it opens: making the window wide doesn't change it.
            b.cdp.call("Emulation.setDeviceMetricsOverride", width=1600, height=800, deviceScaleFactor=1, mobile=False)
            time.sleep(0.3)
            self.assertEqual(b.text(".diffnote-layout__button.is-current"), "統合")
            self.assertEqual(b.count("table.diffnote-diff--split"), 0)
            b.cdp.call("Emulation.setDeviceMetricsOverride", width=1000, height=800, deviceScaleFactor=1, mobile=False)
            time.sleep(0.3)
            self.assertEqual(b.text(".diffnote-layout__button.is-current"), "統合")
        finally:
            b.cdp.call("Emulation.clearDeviceMetricsOverride")

    def test_a_removed_line_sits_beside_the_added_ones_and_unchanged_lines_are_on_both_sides(self):
        b = self.b
        unified = b.count("table.diffnote-diff tr[class*='diffnote-line--']")
        self.split()
        self.assertEqual(b.text(".diffnote-layout__button.is-current"), "横並び")
        self.assertEqual(b.count("table.diffnote-diff:not(.diffnote-diff--split)"), 0)
        rows = b.js("""Array.from(document.querySelectorAll('table.diffnote-diff--split tr.diffnote-split-row')).map(function(tr){
          var kind = function(td){ return td.className.split(' ').filter(function(c){return c.indexOf('diffnote-cell--')===0})[0].slice(15); };
          var c = tr.children;
          return [c[0].textContent, kind(c[0]), c[2].textContent, kind(c[2])];
        })""")
        by_new = {r[2]: r for r in rows}
        # Line 8 of the old file was replaced by 9-13: it sits beside line 9.
        self.assertEqual(by_new["9"][:2], ["8", "removed"])
        self.assertEqual(by_new["9"][3], "added")
        # ...and the lines added after it have nothing on the left.
        for n in ("10", "11", "12", "13"):
            self.assertEqual(by_new[n][:2], ["", "empty"], n)
            self.assertEqual(by_new[n][3], "added")
        # An unchanged line is on both sides, with its number on each.
        self.assertEqual(by_new["1"], ["1", "context", "1", "context"])
        self.assertLess(len(rows), unified, "a removed line shares a row with an added one")

    def test_the_words_that_changed_are_emphasized_in_both_layouts(self):
        b = self.b
        # (A word that spans pieces of code of different kinds is several spans.)
        words = lambda scope: ["".join(b.js("Array.from(document.querySelectorAll('%s .diffnote-word')).map(function(e){return e.textContent})" % scope))]
        self.assertEqual(words(".diffnote-line--removed"), [".pass === pass"])
        self.assertEqual(words(".diffnote-line--added"), ["!"])
        # Its background is stronger than the line's own.
        self.assertNotEqual(
            b.js("getComputedStyle(document.querySelector('.diffnote-line--removed .diffnote-word')).backgroundColor"),
            b.js("getComputedStyle(document.querySelector('.diffnote-line--removed')).backgroundColor"))
        b.js("localStorage.setItem('diffnote-layout','split')")
        b.reload()
        self.assertEqual(words(".diffnote-cell--removed"), [".pass === pass"])
        self.assertEqual(words(".diffnote-cell--added"), ["!"])
        # The text is whole: the words are only wrapped.
        self.assertIn("if (account.pass === pass) {", b.text(".diffnote-cell--removed.diffnote-line__content"))

    def copy_after_dragging(self, side, first, last):
        b = self.b
        cell = lambda n: "document.querySelector(\"td.diffnote-line__gutter-%s[data-diffnote-%s='%d']\").nextElementSibling" % (side, side, n)
        b.js("%s.setAttribute('data-t','from'); %s.setAttribute('data-t','to')" % (cell(first), cell(last)))
        b.drag("[data-t=from]", "[data-t=to]")
        copied = b.js("(function(){var dt=new DataTransfer(); document.dispatchEvent(new ClipboardEvent('copy',{clipboardData:dt,bubbles:true,cancelable:true})); return dt.getData('text/plain')})()")
        if not copied:
            # This has never worked on the CI runner and always works here,
            # so when it fails it must say what the browser thought: whether
            # anything was selected at all, and whether the two cells were
            # where the mouse was told to go.
            print("\n-- nothing copied. selection:", b.js("""(function () {
              var s = getSelection();
              return JSON.stringify({ ranges: s.rangeCount, length: s.toString().length,
                text: s.toString().slice(0, 60) });
            })()"""), file=sys.stderr)
            print("-- where the page is:", b.js("""(function () {
              var box = function (sel) {
                var e = document.querySelector(sel);
                if (!e) return null;
                var r = e.getBoundingClientRect();
                return [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)];
              };
              var at = function (sel) {
                var e = document.querySelector(sel);
                if (!e) return null;
                var r = e.getBoundingClientRect();
                var hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
                return hit ? hit.tagName + '.' + (hit.className || '') : 'nothing';
              };
              return JSON.stringify({ view: [innerWidth, innerHeight], scroll: Math.round(scrollY),
                from: box('[data-t=from]'), to: box('[data-t=to]'),
                atFrom: at('[data-t=from]'), atTo: at('[data-t=to]'),
                ua: navigator.userAgent.replace(/.*Chrome\\/([0-9.]+).*/, 'Chrome $1') });
            })()"""), file=sys.stderr)
        return copied

    def test_copying_after_a_drag_takes_only_the_side_it_started_on(self):
        b = self.b
        b.js("localStorage.setItem('diffnote-layout','split')")
        b.reload()
        # The new side, from the line before the change to the last added one.
        copied = self.copy_after_dragging("new", 8, 12)
        # (Where in a line the press falls depends on how wide the column is.)
        self.assertIn("  if (!account) {\n    return res.status(401).end()\n  }\n  const ok = await compare(pass,", copied)
        # The "@@" row (and cards) in between are not selected either.
        self.assertEqual(b.js("getComputedStyle(document.querySelector('.diffnote-diff--split .diffnote-hunk-header td')).userSelect"), "none")
        self.assertNotIn("account.pass === pass", copied, "the old side is not taken along")
        self.assertNotRegex(copied, r"^\d+\t", "nor are the line numbers")
        # From the old side: its own lines only.
        copied = self.copy_after_dragging("old", 7, 11)
        self.assertIn("if (account.pass === pass) {", copied)
        self.assertNotIn("compare(pass, account.hash)", copied)
        self.assertNotIn("if (!account)", copied)

    def test_cards_and_marks_follow_in_the_split_layout(self):
        b = self.b
        self.split()
        # The thread on new line 2 (`import { compare }`): a mark on the right
        # gutter only, and its card in a full-width row under the pair.
        row = "table.diffnote-diff--split tr.diffnote-split-row[data-diffnote-threads]"
        self.assertEqual(b.count(row), 1)
        self.assertTrue(b.js("(function(){var tr=document.querySelector(%s); return tr.children[3].previousElementSibling.classList.contains('diffnote-gutter--commented') && !tr.children[0].classList.contains('diffnote-gutter--commented')})()" % json.dumps(row)))
        self.assertIn("--diffnote-bars", b.js("document.querySelector(%s).children[2].getAttribute('style')" % json.dumps(row)))
        card = b.js("(function(){var tr=document.querySelector(%s).nextElementSibling; return [tr.className, tr.children[0].colSpan, tr.textContent.includes('hash')]})()" % json.dumps(row))
        self.assertEqual(card, ["diffnote-thread-row", 4, True])
        # Hovering the card shows the row of its range.
        b.hover(".diffnote-thread summary")
        self.assertGreaterEqual(b.count("tr.diffnote-range"), 1)

    def test_the_choice_is_kept_and_a_narrow_window_falls_back_to_unified(self):
        b = self.b
        self.split()
        b.reload()
        self.assertGreaterEqual(b.count("table.diffnote-diff--split"), 1, "kept")
        b.cdp.call("Emulation.setDeviceMetricsOverride", width=700, height=800, deviceScaleFactor=1, mobile=False)
        try:
            self.assertTrue(b.wait("document.querySelectorAll('table.diffnote-diff--split').length === 0"), "no room for two columns")
            self.assertEqual(b.count(".diffnote-layout"), 0, "and no switch")
        finally:
            b.cdp.call("Emulation.clearDeviceMetricsOverride")
        self.assertTrue(b.wait("document.querySelectorAll('table.diffnote-diff--split').length >= 1"), "back with room")
        b.click("[data-diffnote-layout='unified']")
        self.assertTrue(b.wait("document.querySelectorAll('table.diffnote-diff--split').length === 0"))
        b.reload()
        self.assertEqual(b.count("table.diffnote-diff--split"), 0, "unified is kept too")

    def test_a_long_line_wraps_instead_of_making_the_page_scroll_sideways(self):
        b = self.b
        b.open(self.long_url)
        b.js("localStorage.setItem('diffnote-layout','split')")
        b.reload()
        self.assertTrue(b.wait_exists("table.diffnote-diff--split"))
        self.assertLessEqual(b.js("(function(){var s=document.querySelector('.diffnote-diff-scroll'); return s.scrollWidth - s.clientWidth})()"), 1)
        # The changed line is a removed one and an added one on one row, wrapped.
        self.assertGreater(b.js("document.querySelector('table.diffnote-diff--split .diffnote-cell--removed.diffnote-line__content').getBoundingClientRect().height"), 40)
        b.js("localStorage.setItem('diffnote-layout','unified')")
        b.reload()
        self.assertGreater(b.js("(function(){var s=document.querySelector('.diffnote-diff-scroll'); return s.scrollWidth - s.clientWidth})()"), 100, "unified scrolls sideways, as before")


if __name__ == "__main__":
    unittest.main()


class ExpandLeftOutLines(BrowserCase):
    """The lines a diff leaves out, shown a little at a time or all at once."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        review, _ = make_gaps_review(cls.root)
        cls.review = review
        cls.urls = {}
        for name, extra in (("all", []), ("none", ["--expand-limit", "0"]), ("some", ["--expand-limit", "20"])):
            path = os.path.join(cls.root, f"gaps_{name}.html")
            assert diffnote("export", "-f", review, path, *extra).returncode == 0
            cls.urls[name] = pathlib.Path(path).as_uri()

    def open(self, name):
        self.b = self.browser
        self.b.open(self.urls[name])
        self.b.js("localStorage.clear(); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()
        # A diff with no thread on a file starts folded: open the file.
        self.assertTrue(self.b.wait_exists(".diffnote-expand-row"))

    def rows(self):
        return self.b.js("Array.from(document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]')).map(function(r){return +r.getAttribute('data-diffnote-new')})")

    def markers(self):
        return self.b.js("Array.from(document.querySelectorAll('.diffnote-expand-row .diffnote-expand')).map(function(e){return e.textContent.trim().replace(/\\s+/g,' ')})")

    def test_the_places_are_marked_with_how_many_lines_are_left_out(self):
        self.open("all")
        self.assertEqual(self.b.count(".diffnote-expand-row"), 3)
        self.assertTrue(self.b.js("document.querySelectorAll('.diffnote-expand-row')[1].textContent.includes('53')"), self.markers())
        self.assertEqual(self.rows()[0], 17, "the first hunk starts at line 17")

    def test_a_press_shows_a_few_lines_next_to_the_hunk_it_names_and_the_rest_stays_hidden(self):
        self.open("all")
        b = self.b
        middle = ".diffnote-expand-row:nth-of-type(2)"
        before = len(self.rows())
        # The middle place: 53 lines. "↓" shows 20 after the hunk above (23-42).
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('53')})[0].querySelector('[data-diffnote-expand=top]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 20}"))
        rows = self.rows()
        self.assertEqual(rows[rows.index(23) + 19], 42, "the 20 lines after the hunk above")
        self.assertTrue(any("33" in m for m in self.markers()), self.markers())
        # "↑" shows 20 before the hunk below (57-76).
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('33')})[0].querySelector('[data-diffnote-expand=bottom]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 40}"))
        self.assertIn(57, self.rows())
        self.assertNotIn(50, self.rows(), "the middle of the place is still out")

    def test_the_at_at_row_of_a_hunk_is_dropped_once_lines_next_to_it_are_shown(self):
        self.open("all")
        b = self.b
        self.assertEqual(b.count(".diffnote-hunk-header"), 2)
        # "↑" on the middle place shows the 20 lines before the second hunk.
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('53')})[0].querySelector('[data-diffnote-expand=bottom]').click()")
        self.assertTrue(b.wait("document.querySelectorAll('.diffnote-hunk-header').length === 1"))
        # The shown lines run into the hunk: the row before its first line (77)
        # is the shown line 76, not a @@ row.
        before = b.js("document.querySelector('.diffnote-diff tr[data-diffnote-new=\"77\"]').previousElementSibling.getAttribute('data-diffnote-new')")
        self.assertEqual(before, "76")
        # The place is still marked, above the lines shown.
        self.assertEqual(b.count(".diffnote-expand-row"), 3)

    def test_all_at_once_leaves_no_marker_and_the_lines_are_whole_and_in_order(self):
        self.open("all")
        b = self.b
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('53')})[0].querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait("document.querySelectorAll('.diffnote-expand-row').length === 2"))
        rows = self.rows()
        self.assertEqual(rows[rows.index(23):rows.index(77)], list(range(23, 77)), "24-76 all there, in order")
        self.assertEqual(b.count(".diffnote-hunk-header"), 1, "the second hunk's @@ row is not needed")
        self.assertIn("row 50", b.text("table.diffnote-diff"))

    def test_a_place_over_the_limit_is_only_named_and_the_smaller_ones_can_be_shown(self):
        self.open("some")
        markers = self.markers()
        # Limit 20: the places of 16 and of 17 lines fit (not both), the 53 don't.
        buttons = self.b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).map(function(r){return r.querySelectorAll('button').length})")
        self.assertEqual(buttons[1], 0, markers)
        self.assertIn("含まれていません", markers[1])
        self.assertEqual(sum(1 for n in buttons if n > 0), 1, "one of the two smaller places fits in 20")

    def test_with_none_carried_no_place_can_be_shown(self):
        self.open("none")
        self.assertEqual(self.b.count(".diffnote-expand-row button"), 0)
        self.assertEqual(self.b.count(".diffnote-expand-row"), 3)

    def test_shown_lines_are_lines_to_read_in_both_layouts_and_a_thread_can_still_be_read(self):
        self.open("all")
        b = self.b
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('16')})[0].querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait("document.querySelectorAll('.diffnote-expand-row').length === 2"))
        self.assertEqual(self.rows()[:3], [1, 2, 3])
        b.js("localStorage.setItem('diffnote-layout','split')")
        b.reload()
        # The choice of layout starts the places over (they are shown again as markers).
        self.assertTrue(b.wait_exists(".diffnote-expand-row"))
        self.assertEqual(b.count("table.diffnote-diff--split .diffnote-expand-row td[colspan='4']"), 3)


class BinaryFiles(BrowserCase):
    """A binary file has no lines to show what was done to it, so its title says."""

    def test_the_title_says_whether_a_binary_file_was_added_deleted_or_changed(self):
        repo = os.path.join(self.root, "bins")
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        for name in ("gone.bin", "same.bin"):
            with open(os.path.join(repo, name), "wb") as f:
                f.write(b"\xff\xfe\x00\x01")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c1")
        git(repo, "tag", "c1")
        os.remove(os.path.join(repo, "gone.bin"))
        with open(os.path.join(repo, "same.bin"), "wb") as f:
            f.write(b"\xff\xfe\x00\x02")
        with open(os.path.join(repo, "new.bin"), "wb") as f:
            f.write(b"\xff\x00\x03")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c2")
        git(repo, "tag", "c2")
        review = os.path.join(self.root, "bins.diffnote")
        assert diffnote("edit", "-f", review, "--base", "c1", "c2", cwd=repo,
                        comments=[("GLOBAL", "binary")]).returncode == 0
        html = os.path.join(self.root, "bins.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        b = self.browser
        b.open(pathlib.Path(html).as_uri(), ready="!!document.querySelector('.diffnote-file')")
        titles = b.js("[...document.querySelectorAll('section.diffnote-file h2')].map(h => h.textContent).join('|')")
        self.assertIn("gone.bin (バイナリ・削除)", titles)
        self.assertIn("new.bin (バイナリ・追加)", titles)
        self.assertIn("same.bin (バイナリ・変更)", titles)
        self.assertEqual(b.count("[data-diffnote-binary]"), 3, "each binary file says why nothing is shown")
        self.assertEqual(b.count("[data-diffnote-stat]"), 0, "no lines to count")
        self.assertIn("バイナリ", b.js("document.querySelector('[data-diffnote-binary]').textContent"))
        tint = lambda name: b.js("getComputedStyle(document.querySelector('section.diffnote-file[data-diffnote-file=\"%s\"] summary')).backgroundColor" % name)
        self.assertEqual(tint("new.bin"), "rgb(230, 255, 236)", "added: greenish")
        self.assertEqual(tint("gone.bin"), "rgb(255, 235, 233)", "deleted: reddish")
        self.assertNotIn(tint("same.bin"), (tint("new.bin"), tint("gone.bin")), "changed: as usual")



class RenamedFiles(BrowserCase):
    """A renamed file is listed under the name it has now, so its heading is
    the one place that says where it was before -- and one that moved and was
    not otherwise touched has nothing in the diff at all."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        repo = os.path.join(cls.root, "moved")
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        # Long enough that git still sees the same file after the change
        # (it calls a rename by how alike the two sides are).
        cls.before = "".join("line %d\n" % n for n in range(1, 13))
        write(repo, "src/old.py", cls.before)
        write(repo, "src/stays.py", "GREETING = 'hello'\n")
        write(repo, "kept.py", "x = 1\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c1")
        git(repo, "tag", "c1")
        os.makedirs(os.path.join(repo, "src", "deeper"))
        git(repo, "mv", "src/old.py", "src/deeper/new.py")
        write(repo, "src/deeper/new.py", cls.before.replace("line 7\n", "line seven\n"))
        # Moved and nothing else: the diff has the file and none of its lines.
        git(repo, "mv", "src/stays.py", "src/deeper/stays.py")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c2")
        git(repo, "tag", "c2")
        review = os.path.join(cls.root, "moved.diffnote")
        assert diffnote("edit", "-f", review, "--base", "c1", "c2", cwd=repo,
                        comments=[("+line seven", "数字のままでよいのでは。")]).returncode == 0
        html = os.path.join(cls.root, "moved.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        cls.url = pathlib.Path(html).as_uri()

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url, ready="!!document.querySelector('.diffnote-file')")

    def test_the_title_says_the_path_a_renamed_file_came_from(self):
        b = self.b
        moved = 'section.diffnote-file[data-diffnote-file="src/deeper/new.py"]'
        self.assertEqual(b.text(moved + " h2"), "src/deeper/new.py (src/old.py から名前変更)")
        self.assertEqual(
            b.js("document.querySelector(%s).getAttribute('data-diffnote-renamed-from')" % json.dumps(moved + " [data-diffnote-renamed-from]")),
            "src/old.py", "the path on its own, for whatever wants it")
        self.assertEqual(b.count("[data-diffnote-renamed-from]"), 2, "the two that moved, and nothing else")
        self.assertEqual(
            b.js("document.querySelector(%s).getAttribute('data-diffnote-copy')" % json.dumps(moved + " .diffnote-copy")),
            "src/deeper/new.py", "the copy button is still the path it has now")
        # Folded, it still says it: that is what the heading is for.
        b.js("document.querySelector(%s).open = false" % json.dumps(moved + " details"))
        self.assertTrue(b.visible(moved + " [data-diffnote-renamed-from]"))

    def test_a_file_that_only_moved_can_still_be_read(self):
        # Its diff is empty, and the tree of other files lists only what the
        # diff doesn't have -- so without this there is no way to its content.
        b = self.b
        moved = 'section.diffnote-file[data-diffnote-file="src/deeper/stays.py"]'
        self.assertEqual(b.text(moved + " h2"), "src/deeper/stays.py (src/stays.py から名前変更)")
        self.assertEqual(b.count(moved + " [data-diffnote-stat]"), 0, "no lines changed to count")
        b.js("var d = document.querySelector(%s); d.open = true; d.dispatchEvent(new Event('toggle'))"
             % json.dumps(moved + " details"))
        self.assertTrue(b.wait("!!document.querySelector(%s)" % json.dumps(moved + " [data-diffnote-expand]")),
                        "the whole file is one place to open")
        b.click(moved + " [data-diffnote-expand='all']")
        self.assertTrue(b.wait("document.querySelectorAll(%s).length === 1" % json.dumps(moved + " tbody tr")))
        self.assertIn("GREETING = 'hello'", b.text(moved + " tbody tr"))
        self.assertEqual(b.count(moved + " [data-diffnote-expand]"), 0, "nothing left to open")


class LineLinks(BrowserCase):
    """Places in a comment that name lines of a file go to those lines."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        repo = os.path.join(cls.root, "links")
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        text = lambda a, b: "".join({5: a + "\n", 40: b + "\n"}.get(n, f"row {n}\n") for n in range(1, 61))
        write(repo, "long.txt", text("five", "forty"))
        write(repo, "other.txt", "x\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c1")
        git(repo, "tag", "c1")
        write(repo, "long.txt", text("FIVE", "FORTY"))
        write(repo, "other.txt", "y\n")
        git(repo, "commit", "-q", "-am", "c2")
        git(repo, "tag", "c2")
        review = os.path.join(cls.root, "links.diffnote")
        assert diffnote("edit", "-f", review, "--base", "c1", "c2", cwd=repo, comments=[
            ("+FIVE", "ここは long.txt:40-41 と other.txt:1 と対で、time 12:30 や none.txt:4 は対象外です。削除は long.txt:L5@1 です。"),
        ]).returncode == 0
        write(repo, "other.txt", "z\n")
        git(repo, "commit", "-q", "-am", "c3")
        git(repo, "tag", "c3")
        out = diffnote("edit", "-f", review, "--base", "c1", "c3", cwd=repo, comments=[("GLOBAL", "二つ目")])
        assert out.returncode == 0, out.stdout + out.stderr
        html = os.path.join(cls.root, "links.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        cls.url = pathlib.Path(html).as_uri()

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url)
        self.b.js("localStorage.clear(); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()

    def test_only_the_places_that_are_files_of_the_review_are_links(self):
        b = self.b
        self.assertEqual(b.js("[...document.querySelectorAll('[data-diffnote-lineref]')].map(a => a.textContent).join('|')"), "long.txt:40-41|other.txt:1|long.txt:L5@1")

    def test_pressing_one_goes_to_the_lines_and_marks_them(self):
        b = self.b
        b.click("[data-diffnote-lineref]")
        self.assertTrue(b.wait_exists("tr.diffnote-linked"))
        self.assertEqual(b.js("document.querySelector('tr.diffnote-linked').getAttribute('data-diffnote-new')"), "40")
        self.assertTrue(b.wait("!document.querySelector('tr.diffnote-linked')"), "the mark goes away")

    def test_it_brings_back_a_file_that_was_looked_at_and_shows_it(self):
        b = self.b
        section = f"{CUR} section.diffnote-file[data-diffnote-file='other.txt']"
        b.click(f"{section} [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)})"))
        b.click("[data-diffnote-lineref='other.txt:1-1']")
        self.assertTrue(b.wait_exists(section), "the file is back")
        self.assertTrue(b.wait_exists("tr.diffnote-linked"))
        self.assertFalse(b.exists("[data-diffnote-check='other.txt'][aria-pressed='true']"))

    def test_a_link_to_removed_lines_of_another_revision_switches_to_it_and_marks_them(self):
        b = self.b
        self.assertEqual(b.js("document.querySelector('.diffnote-revision.is-current').id"), "rev-1", "the latest is shown")
        title = b.js("document.querySelector(\"[data-diffnote-lineref='long.txt:L5-5']\").title")
        self.assertIn("#1", title)
        self.assertIn("最新のリビジョンではありません", title)
        b.click("[data-diffnote-lineref='long.txt:L5-5']")
        self.assertTrue(b.wait("document.querySelector('.diffnote-revision.is-current').id === 'rev-0'"), "switched to #1")
        self.assertTrue(b.wait_exists("tr.diffnote-linked"))
        self.assertEqual(b.js("document.querySelector('tr.diffnote-linked').getAttribute('data-diffnote-old')"), "5")
        self.assertIn("removed", b.js("document.querySelector('tr.diffnote-linked').className"), "it is the removed line")

    def test_the_copy_buttons_of_a_thread_carry_the_revision(self):
        b = self.b
        self.assertEqual(b.js("document.querySelector(`${'%s'} .diffnote-thread .diffnote-copy`).getAttribute('data-diffnote-copy')" % CUR).endswith("@2"), True)

    def test_the_browser_back_and_forward_buttons_retrace_a_jump_to_another_revision(self):
        # Exported HTML is opened from a file (no server): the address's hash is
        # the only part of the URL that can change without a real navigation.
        b = self.b
        self.assertEqual(b.js("document.querySelector('.diffnote-revision.is-current').id"), "rev-1")
        b.click("[data-diffnote-lineref='long.txt:L5-5']")
        self.assertTrue(b.wait("document.querySelector('.diffnote-revision.is-current').id === 'rev-0'"))
        # A moment for the popstate listener (a useEffect) to attach.
        time.sleep(0.1)
        b.js("history.back()")
        self.assertTrue(b.wait("document.querySelector('.diffnote-revision.is-current').id === 'rev-1'"))
        time.sleep(0.1)
        b.js("history.forward()")
        self.assertTrue(b.wait("document.querySelector('.diffnote-revision.is-current').id === 'rev-0'"))

    def test_picking_a_file_in_the_list_brings_it_back_opens_it_and_marks_it(self):
        b = self.b
        section = f"{CUR} section.diffnote-file[data-diffnote-file='other.txt']"
        self.assertFalse(b.js(f"document.querySelector({json.dumps(section + ' details')}).open"), "it starts folded")
        # Folded: picking it opens it.
        b.click("[data-diffnote-file-link='other.txt']")
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(section + ' details')}).open"))
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(section)}).classList.contains('diffnote-flash')"), "marked")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)}).classList.contains('diffnote-flash')"), "and not for long")
        # Looked at: picking it takes that back too.
        b.click(f"{section} [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)})"))
        b.click("[data-diffnote-file-link='other.txt']")
        self.assertTrue(b.wait(f"!!document.querySelector({json.dumps(section)})"))
        self.assertFalse(b.exists("[data-diffnote-check='other.txt'][aria-pressed='true']"))
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(section + ' details')}).open"))
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(section)}).classList.contains('diffnote-flash')"))


class IgnoreWhitespace(BrowserCase):
    """Lines that differ only in white space, shown as unchanged when asked."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.review, _ = make_indent_review(cls.root)
        html = os.path.join(cls.root, "indent.html")
        assert diffnote("export", "-f", cls.review, html).returncode == 0
        cls.url = pathlib.Path(html).as_uri()

    def rows(self):
        b = self.b
        return (b.count("tr.diffnote-line--removed"), b.count("tr.diffnote-line--added"))

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url)
        self.b.js("localStorage.clear(); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()

    def test_the_file_says_how_many_lines_it_adds_and_removes_as_they_are_shown(self):
        b = self.b
        stat = lambda: b.js("document.querySelector('[data-diffnote-stat]').textContent.replace(/\\s+/g, ' ').trim()")
        self.assertEqual(stat(), "+3 −3")
        self.assertEqual(b.count("[data-diffnote-stat] .diffnote-stat__blocks i"), 5)
        self.assertEqual(b.count("[data-diffnote-stat] i.is-a"), 3)
        self.assertEqual(b.count("[data-diffnote-stat] i.is-d"), 2)
        self.assertEqual(b.js("document.querySelector('[data-diffnote-stat]').title"), "追加 3 行、削除 3 行")
        # At the right of the header, before the button that says the file was looked at.
        pos = b.js("(() => { const s = document.querySelector('[data-diffnote-stat]').getBoundingClientRect(); const h = document.querySelector('section.diffnote-file summary').getBoundingClientRect(); return h.right - s.right; })()")
        self.assertLess(pos, 200)
        # With white space ignored, only what really changed is counted.
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-stat]').textContent.replace(/\\s+/g, ' ').trim() === '+1 −1'"))

    def test_the_lines_changed_only_in_white_space_are_shown_as_unchanged_when_asked(self):
        b = self.b
        self.assertEqual(self.rows(), (3, 3))
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('tr.diffnote-line--removed').length === 1"))
        self.assertEqual(self.rows(), (1, 1), "only c is a change")
        self.assertEqual(b.count("tr.diffnote-line--context"), 3, "def, a and b")
        # The thread is still on its line.
        self.assertTrue(b.js("document.querySelector('tr.diffnote-line--added').nextElementSibling.classList.contains('diffnote-thread-row')"))
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('tr.diffnote-line--removed').length === 3"))

    def test_a_review_that_says_to_ignore_it_is_exported_so_and_can_still_be_switched(self):
        review = os.path.join(self.root, "indent-default.diffnote")
        shutil.copy(self.review, review)
        add_settings(review, {"ignore_whitespace": True})
        html = os.path.join(self.root, "indent-default.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        b = self.b
        b.open(pathlib.Path(html).as_uri())
        b.js("localStorage.setItem('diffnote-layout','unified')")
        b.reload()
        self.assertTrue(b.js("document.querySelector('[data-diffnote-ignore-space]').checked"))
        self.assertEqual(self.rows(), (1, 1))
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('tr.diffnote-line--removed').length === 3"))

    def test_it_works_side_by_side_too(self):
        b = self.b
        b.click("[data-diffnote-layout='split']")
        self.assertTrue(b.wait_exists("table.diffnote-diff--split"))
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('td.diffnote-cell--removed.diffnote-line__content').length === 1"))


class ViewedFilesAndTheList(BrowserCase):
    """A file marked as looked at is not in the page at all: the list must not
    go on showing it as one of the files on screen."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        repo = os.path.join(cls.root, "two")
        os.makedirs(repo)
        git(repo, "init", "-q", "-b", "main")
        write(repo, "a.txt", "one\n")
        write(repo, "b.txt", "one\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "c1")
        git(repo, "tag", "c1")
        write(repo, "a.txt", "two\n")
        write(repo, "b.txt", "two\n")
        git(repo, "commit", "-q", "-am", "c2")
        git(repo, "tag", "c2")
        review = os.path.join(cls.root, "two.diffnote")
        harness.set_user_author("reviewer")
        out = diffnote("edit", "-f", review, "--base", "c1", "c2", cwd=repo, comments=[("+two", "見ました")])
        assert out.returncode == 0, out.stdout + out.stderr
        html = os.path.join(cls.root, "two.html")
        assert diffnote("export", "-f", review, html).returncode == 0
        cls.url = pathlib.Path(html).as_uri()

    def test_one_that_has_gone_is_not_left_marked_as_on_screen(self):
        b = self.browser
        b.open(self.url)
        link = lambda path: json.dumps(f"{CUR} .diffnote-filelist a[data-diffnote-file-link='{path}']")
        section = lambda path: json.dumps(f"{CUR} section.diffnote-file[data-diffnote-file='{path}']")
        b.click(f"{CUR} section.diffnote-file[data-diffnote-file='a.txt'] [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({section('a.txt')})"))
        # As if the observer that watches the files never got to say that this
        # one had gone (it is disconnected as the page draws itself again).
        b.js(f"document.querySelector({link('a.txt')}).classList.add('is-visible')")
        # Drawing the list again rebuilds the marks from what is in the page.
        b.click(f"{CUR} section.diffnote-file[data-diffnote-file='b.txt'] [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({section('b.txt')})"))
        self.assertTrue(b.wait(f"!document.querySelector({link('a.txt')}).classList.contains('is-visible')"),
                        "a file that is not in the page is not one of the files on screen")
