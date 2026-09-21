"""The exported page (opened from a file, no server)."""
import os
import pathlib
import unittest

import harness
from harness import BrowserCase, diffnote, make_calc_review

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

    def test_the_page_fits_a_narrow_screen(self):
        b = self.b
        b.cdp.call("Emulation.setDeviceMetricsOverride", width=500, height=800, deviceScaleFactor=1, mobile=False)
        try:
            b.reload()
            self.assertLessEqual(b.js("document.documentElement.scrollWidth"), b.js("document.documentElement.clientWidth"))
        finally:
            b.cdp.call("Emulation.clearDeviceMetricsOverride")


if __name__ == "__main__":
    unittest.main()
