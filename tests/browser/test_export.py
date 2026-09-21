"""The exported page (opened from a file, no server)."""
import json
import os
import pathlib
import unittest

import harness
import harness
from harness import BrowserCase, diffnote, git, make_calc_review, make_login_review, write

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
        # Every test starts as a first visit: nothing kept by the browser.
        self.b.js("localStorage.clear()")
        self.b.reload()

    def card(self, text):
        """Mark the visible thread card that says `text`, so the mouse can find it."""
        self.b.js("""(function(){
          document.querySelectorAll('[data-test-card]').forEach(function(e){e.removeAttribute('data-test-card')});
          var c = Array.from(document.querySelectorAll('%s .diffnote-thread')).find(function(t){return t.textContent.includes(%s)});
          c.setAttribute('data-test-card','1'); c.scrollIntoView({block:'center'});
        })()""" % (CUR, repr(text)))
        return "[data-test-card] summary"

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
        self.assertEqual(b.js("window.__copied"), where)

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
        assert diffnote("edit", "-f", long_review, "c1..c2", cwd=repo,
                        comments=[("+" + "word " * 100 + "two", "wide")]).returncode == 0
        long_html = os.path.join(cls.root, "long.html")
        assert diffnote("export", "-f", long_review, long_html).returncode == 0
        cls.long_url = pathlib.Path(long_html).as_uri()

    def setUp(self):
        self.b = self.browser
        self.b.open(self.url)
        self.b.js("localStorage.clear()")
        self.b.reload()

    def split(self):
        self.b.click("[data-diffnote-layout='split']")
        self.assertTrue(self.b.wait_exists("table.diffnote-diff--split"))

    def test_the_switch_offers_two_layouts_and_starts_unified(self):
        b = self.b
        self.assertEqual(b.count(".diffnote-layout__button"), 2)
        self.assertEqual(b.text(".diffnote-layout__button.is-current"), "統合")
        self.assertEqual(b.count("table.diffnote-diff--split"), 0)
        self.assertGreater(b.count("table.diffnote-diff tr[class*='diffnote-line--']"), 0)

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
