"""`diffnote serve`'s page drawn by the client app (`/next`): replies,
resolving, the version check, shutting down."""
import unittest

import json
import time

from harness import BrowserCase, Served, make_calc_review, make_login_review, show
import os
import shutil

CUR = ".diffnote-revision.is-current"


class ClientServed(BrowserCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.calc, _ = make_calc_review(cls.root)
        cls.login, _ = make_login_review(cls.root)

    def serve(self, master=None):
        self.review = os.path.join(self.fresh("review"), "r.diffnote")
        shutil.copy(master or self.calc, self.review)
        self.server = Served(self.review, author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        # The token cookie comes with the first visit to `/`.
        self.b.open(self.server.url)
        self.b.open(self.server.url.split("/?")[0] + "/next")
        self.b.js("localStorage.setItem('diffnote-hide-resolved','0')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")

    def write(self, selector, text):
        """Types into a box the way the page hears it (and lets it settle)."""
        self.b.js("var t=document.querySelector(%r); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,%r); t.dispatchEvent(new Event('input',{bubbles:true}))" % (selector, text))
        time.sleep(0.1)

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


LOGIN = "table[data-diffnote-file='src/auth/login.ts']"


class NewThreadsOnLines(ClientServed):
    def gutter(self, kind, n, table=LOGIN):
        return f"{CUR} {table} tr[data-diffnote-{kind}='{n}'] .diffnote-line__gutter-{kind}"

    def send_box(self, text):
        self.write(".diffnote-composer-row textarea", text)
        self.b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")

    def test_pressing_a_line_number_opens_a_box_and_the_thread_is_put_in_place(self):
        self.serve(self.login)
        b = self.b
        before = b.count(f"{CUR} .diffnote-thread")
        b.click_at(self.gutter("new", 6))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:6")
        self.assertTrue(b.wait("document.activeElement.tagName==='TEXTAREA'"))
        self.send_box("1 行へのコメント")
        self.assertTrue(b.wait(f"!document.querySelector('.diffnote-composer-row') && document.querySelectorAll('{CUR} .diffnote-thread').length === {before + 1}"))
        self.assertTrue(self.same_page())
        row = self.gutter("new", 6)
        self.assertTrue(b.js(f"(function(){{var r=document.querySelector({row!r}).closest('tr'); var n=r.nextElementSibling; return r.classList.contains('diffnote-line--commented') && n.classList.contains('diffnote-thread-row') && n.textContent.includes('1 行へのコメント')}})()"))
        self.assertEqual(self.counts(), f"スレッド {before + 1} 件(解決済み 0 件)")
        self.assertTrue(any("login.ts:6" in l for l in show(self.review).splitlines() if "新規" in l))

    def test_dragging_over_removed_and_added_lines_chooses_them_all(self):
        self.serve(self.login)
        b = self.b
        b.drag(self.gutter("old", 8), self.gutter("new", 12))
        self.assertEqual(b.count(".diffnote-select"), 5, "the removed line and four added ones")
        self.assertEqual(b.count(".diffnote-select-first"), 1)
        self.assertEqual(b.count(".diffnote-select-last"), 1)
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:9-12")
        self.send_box("ドラッグで選んだ範囲")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("login.ts:9-12 <- src/auth/login.ts:8" in l for l in show(self.review).splitlines()), show(self.review))

    def test_shift_click_extends_the_choice_and_a_draft_survives_choosing_again(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 2))
        b.click_at(self.gutter("new", 4), modifiers=8)
        self.assertEqual(b.count(".diffnote-select"), 3)
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:2-4")
        self.write(".diffnote-composer-row textarea", "書きかけ")
        b.click_at(self.gutter("new", 5))
        self.assertEqual(b.count(".diffnote-composer-row"), 1)
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "書きかけ")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.diffnote-select')"))

    def test_no_thread_is_highlighted_while_lines_are_being_chosen(self):
        self.serve(self.login)
        b = self.b
        b.hover(f"{CUR} .diffnote-thread summary")
        self.assertGreaterEqual(b.count(".diffnote-range"), 1)
        b.press(self.gutter("new", 6))
        self.assertEqual(b.count(".diffnote-range"), 0)
        for n in (5, 4, 3, 2):
            x, y = b.center(self.gutter("new", n))
            b.cdp.mouse("mouseMoved", x, y, 1)
            self.assertEqual(b.count(".diffnote-range"), 0, n)
            self.assertEqual(b.count(".diffnote-thread.diffnote-hover"), 0, n)
        b.release(self.gutter("new", 2))
        self.assertEqual(b.count(".diffnote-select-first"), 1)

    def test_a_failed_send_keeps_the_box_and_the_words(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 6))
        self.write(".diffnote-composer-row textarea", "止まった後")
        self.server.shut_down(b)
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!!document.querySelector('.diffnote-compose .diffnote-error')"))
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "止まった後")
        self.assertTrue(b.js("document.querySelector('.diffnote-compose').style.display !== 'none'"))

    def test_the_thread_is_followed_into_the_other_revision(self):
        self.serve(self.calc)
        b = self.b
        b.click_at(f"{CUR} table[data-diffnote-file='calc.py'] tr[data-diffnote-new='14'] .diffnote-line__gutter-new")
        self.send_box("mul の戻り値を確認")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        b.click("[data-diffnote-revision-link='0']")
        where = b.js("""(function(){var t=Array.from(document.querySelectorAll('#rev-0 .diffnote-thread')).find(function(t){return t.textContent.includes('mul の戻り値を確認')}); return t.querySelector('.diffnote-thread__where').textContent})()""")
        self.assertEqual(where, "calc.py:12", "line 14 of the new revision is line 12 of the old one")
        self.assertGreater(b.count("#rev-0 .diffnote-reply"), 0, "still interactive")


class ThreadsOnFilesAndTheReview(ClientServed):
    def test_a_review_wide_thread_is_added_to_its_place(self):
        self.serve()
        b = self.b
        before = b.count(f"{CUR} [data-diffnote-global] .diffnote-thread")
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.assertEqual(b.text(".diffnote-compose__where"), "レビュー全体へのコメント")
        self.write(".diffnote-compose textarea", "全体の方針について")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait(f"document.querySelectorAll('{CUR} [data-diffnote-global] .diffnote-thread').length === {before + 1} && !document.querySelector('.diffnote-compose-wrap')"))
        self.assertTrue(self.same_page())
        self.assertIn("全体の方針について", show(self.review))

    def test_a_file_thread_is_added_to_the_file_and_the_box_can_be_closed_with_escape(self):
        self.serve()
        b = self.b
        file = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py']"
        b.click(f"{file} [data-diffnote-add=file]")
        self.assertEqual(b.text(".diffnote-compose__where"), "calc.py へのコメント")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-compose-wrap')"))
        b.click(f"{file} [data-diffnote-add=file]")
        self.write(".diffnote-compose textarea", "ファイル全体について")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        cards = f"{file} > details > .diffnote-thread"
        self.assertTrue(b.wait(f"document.querySelectorAll({json.dumps(cards)}).length === 1"))
        self.assertEqual(b.text(cards + " .diffnote-thread__where"), "calc.py")
        self.assertTrue(any("ファイル全体: calc.py" in l for l in show(self.review).splitlines()))


if __name__ == "__main__":
    unittest.main()
