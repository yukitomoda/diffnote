"""`diffnote serve`'s page drawn by the client app (`/next`): replies,
resolving, the version check, shutting down."""
import unittest

from harness import BrowserCase, Served, make_calc_review, show
import os
import shutil

CUR = ".diffnote-revision.is-current"


class ClientServed(BrowserCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.calc, _ = make_calc_review(cls.root)

    def serve(self):
        self.review = os.path.join(self.fresh("review"), "r.diffnote")
        shutil.copy(self.calc, self.review)
        self.server = Served(self.review, author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        # The token cookie comes with the first visit to `/`.
        self.b.open(self.server.url)
        self.b.open(self.server.url.split("/?")[0] + "/next")
        self.b.js("localStorage.setItem('diffnote-hide-resolved','0')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")

    def same_page(self):
        return self.b.js("window.__marker==='same-page' && window.__table===document.querySelector('.diffnote-diff')")

    def card(self, text):
        return self.b.js("""(function(){var c=Array.from(document.querySelectorAll('%s .diffnote-thread')).find(function(t){return t.textContent.includes(%r)}); return c.id})()""" % (CUR, text))

    def counts(self):
        return self.b.js("document.querySelector('.diffnote-summary p').textContent")

    def test_a_reply_shows_at_once_and_is_kept(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        b.js(f"var c=document.getElementById({card!r}); var t=c.querySelector('textarea'); var set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(t,'返信のテスト'); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('返信のテスト') && !document.querySelector('.is-pending')"))
        self.assertTrue(self.same_page())
        authors = b.js(f"Array.from(document.getElementById({card!r}).querySelectorAll('.diffnote-comment__author')).map(function(a){{return a.textContent}})")
        self.assertTrue(authors[-1].startswith("検証者"), authors)
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "")
        self.assertIn("返信のテスト", show(self.review))

    def test_resolving_and_reopening_change_the_card_and_the_counts(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)")
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 2 件)")
        self.assertTrue(self.same_page())
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)")
        self.assertIn("再オープン", show(self.review))

    def test_a_failed_change_says_so_and_keeps_the_draft(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        b.js(f"var t=document.getElementById({card!r}).querySelector('textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,'止まった後'); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        self.server.shut_down(b)
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('.diffnote-error')"))
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('.diffnote-error').textContent"), "サーバーに接続できませんでした")
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "止まった後")

    def outside_change(self, card):
        """A change the page doesn't know of (as from another tab)."""
        thread = self.b.js(f"document.getElementById({card!r}).dataset.diffnoteThreadId")
        self.b.js("Diffnote.api.post('/api/threads/%s/replies',{body:'別のタブから'})" % thread)

    def test_coming_back_to_the_window_shows_what_changed_meanwhile(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.outside_change(card)
        self.assertFalse(b.js(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))
        b.js("window.dispatchEvent(new Event('focus'))")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))
        self.assertTrue(self.same_page())

    def test_a_change_made_over_a_stale_page_brings_the_rest_along(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.outside_change(card)
        b.js("var ids=Array.from(document.querySelectorAll('%s .diffnote-thread')).map(function(t){return t.id}); window.__other=ids.find(function(i){return i!==%r})" % (CUR, card))
        b.js("var c=document.getElementById(window.__other); c.querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))

    def test_the_shutdown_button_stops_the_server(self):
        self.serve()
        self.b.js("document.querySelector('[data-diffnote-shutdown]').click()")
        self.assertTrue(self.b.wait("document.body.textContent.includes('終了しました')"))


if __name__ == "__main__":
    unittest.main()
