"""`diffnote serve`: changing a review from the page, in place."""
import json
import os
import shutil
import unittest

import harness
from harness import (BrowserCase, Served, entries, make_calc_review, make_login_review, show)

CUR = ".diffnote-revision.is-current"
LOGIN = "table[data-diffnote-file='src/auth/login.ts']"


class ServedCase(BrowserCase):
    """Each test gets its own copy of a review and its own server."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.calc, _ = make_calc_review(cls.root)
        cls.login, _ = make_login_review(cls.root)

    def serve(self, master):
        self.review = os.path.join(self.fresh("review"), "r.diffnote")
        shutil.copy(master, self.review)
        self.server = Served(self.review, author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        self.b.open(self.server.url)
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff'); window.scrollTo(0,0)")
        # The choices of a first visit: resolved threads shown, to see them change.
        self.b.js("localStorage.setItem('diffnote-hide-resolved','0')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")

    def same_page(self):
        return self.b.js("window.__marker==='same-page' && window.__table===document.querySelector('.diffnote-diff')")

    def gutter(self, kind, n, table=LOGIN):
        return f"{CUR} {table} tr[data-diffnote-{kind}='{n}'] .diffnote-line__gutter-{'new' if kind == 'new' else 'old'}"

    def type_and_send(self, form_selector, text):
        self.b.js("var f=document.querySelector(%r); f.querySelector('textarea').value=%r; window.__t0=performance.now(); f.requestSubmit()" % (form_selector, text))


class Replies(ServedCase):
    def card_id(self, text):
        return self.b.js("""(function(){var c=Array.from(document.querySelectorAll('%s .diffnote-thread')).find(function(t){return t.textContent.includes(%r)}); return c.id})()""" % (CUR, text))

    def test_a_reply_shows_at_once_and_is_kept_without_reloading(self):
        self.serve(self.calc)
        b = self.b
        card = self.card_id("mul の型")
        b.js(f"var c=document.getElementById({card!r}); c.querySelector('textarea').value='返信のテスト'; window.__t0=performance.now(); c.querySelector('form.diffnote-reply').requestSubmit(); window.__pending=!!c.querySelector('.diffnote-comment.is-pending')")
        self.assertTrue(b.js("window.__pending"), "a faded comment at once")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('返信のテスト') && !document.querySelector('.is-pending')"))
        self.assertLess(b.js("performance.now()-window.__t0"), 2000)
        self.assertTrue(self.same_page())
        authors = b.js(f"Array.from(document.getElementById({card!r}).querySelectorAll('.diffnote-comment__author')).map(function(a){{return a.textContent}})")
        self.assertTrue(authors[-1].startswith("検証者"), authors)
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "", "ready for the next")
        self.assertIn("返信のテスト", show(self.review))

    def test_resolving_and_reopening_change_the_card_the_list_and_the_counts(self):
        self.serve(self.calc)
        b = self.b
        card = self.card_id("mul の型")
        counts = lambda: b.js("document.querySelector('.diffnote-summary p').textContent")
        self.assertEqual(counts(), "スレッド 4 件(解決済み 1 件)")
        b.js(f"var c=document.getElementById({card!r}); c.querySelector('[data-diffnote-action]').click(); window.__now=c.classList.contains('diffnote-thread--resolved')")
        self.assertTrue(b.js("window.__now"), "the state shows at once")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        self.assertEqual(counts(), "スレッド 4 件(解決済み 2 件)")
        self.assertEqual(b.js(f"document.querySelector('{CUR} .diffnote-side .diffnote-badge[title]').textContent"), "2 / 4")
        self.assertEqual(b.js(f"document.querySelectorAll('{CUR} .diffnote-threadlist .is-resolved').length"), 2)
        self.assertTrue(self.same_page())
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))
        self.assertEqual(counts(), "スレッド 4 件(解決済み 1 件)")
        self.assertIn("再オープン", show(self.review))

    def test_resolving_while_resolved_threads_are_hidden_makes_the_card_vanish(self):
        self.serve(self.calc)
        b = self.b
        b.js("localStorage.setItem('diffnote-hide-resolved','1')")
        b.reload()
        b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread--resolved"), 0)
        shown = b.visible(f"{CUR} .diffnote-thread")
        card = self.card_id("mul の型")
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).classList.contains('diffnote-thread--resolved')"))
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        self.assertEqual(b.visible(f"{CUR} .diffnote-thread"), shown - 1)
        self.assertEqual(b.visible(f"{CUR} .diffnote-threadlist li"), shown - 1)
        self.assertEqual(b.text("[data-diffnote-resolved-count]"), "(2)")
        # A line that only resolved threads are about loses its mark.
        self.assertGreaterEqual(b.count(f"{CUR} .diffnote-line--resolved-only"), 1)
        self.assertTrue(self.same_page())

    def test_a_failed_change_says_so_and_keeps_the_draft(self):
        self.serve(self.calc)
        b = self.b
        card = self.card_id("mul の型")
        b.js(f"document.getElementById({card!r}).querySelector('textarea').value='サーバーが止まった後'")
        self.server.shut_down(b)
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('.diffnote-error')"))
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('.diffnote-error').textContent"), "サーバーに接続できませんでした")
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "サーバーが止まった後")
        self.assertFalse(b.js("!!document.querySelector('.is-pending')"))


class NewThreadsOnLines(ServedCase):
    def submit_box(self, text):
        self.type_and_send(".diffnote-composer-row .diffnote-compose", text)

    def test_pressing_a_line_number_opens_a_box_and_the_thread_is_put_in_place(self):
        self.serve(self.login)
        b = self.b
        before = b.js(f"document.querySelectorAll('{CUR} .diffnote-thread').length")
        b.click_at(self.gutter("new", 6))
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__where').textContent"), "src/auth/login.ts:6")
        self.assertEqual(b.js("document.activeElement.tagName"), "TEXTAREA")
        self.submit_box("1 行へのコメント")
        self.assertTrue(b.js("!!document.querySelector('.diffnote-composer-row .is-pending')"), "at once")
        self.assertTrue(b.wait(f"!document.querySelector('.diffnote-composer-row') && document.querySelectorAll('{CUR} .diffnote-thread').length === {before + 1}"))
        self.assertTrue(self.same_page())
        row = self.gutter("new", 6)
        self.assertTrue(b.js(f"(function(){{var r=document.querySelector({row!r}).closest('tr'); var n=r.nextElementSibling; return r.classList.contains('diffnote-line--commented') && n.classList.contains('diffnote-thread-row') && n.textContent.includes('1 行へのコメント')}})()"))
        self.assertEqual(b.js("document.querySelector('.diffnote-summary p').textContent"), f"スレッド {before + 1} 件(解決済み 0 件)")
        self.assertTrue(any("login.ts:6" in l for l in show(self.review).splitlines() if "新規" in l))

    def test_dragging_over_removed_and_added_lines_chooses_them_all(self):
        self.serve(self.login)
        b = self.b
        b.drag(self.gutter("old", 8, LOGIN).replace("gutter-new", "gutter-old"), self.gutter("new", 12))
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-select').length"), 5, "the removed line and four added ones")
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-select-first').length"), 1)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-select-last').length"), 1)
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__where').textContent"), "src/auth/login.ts:9-12")
        self.submit_box("ドラッグで選んだ範囲")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("login.ts:9-12 <- src/auth/login.ts:8" in l for l in show(self.review).splitlines()), show(self.review))

    def test_shift_click_extends_the_choice_and_a_draft_survives_choosing_again(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 2))
        b.click_at(self.gutter("new", 4), modifiers=8)
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-select').length"), 3)
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__where').textContent"), "src/auth/login.ts:2-4")
        b.js("document.querySelector('.diffnote-composer-row textarea').value='書きかけ'")
        b.click_at(self.gutter("new", 5))
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-composer-row').length"), 1)
        self.assertEqual(b.js("document.querySelector('.diffnote-composer-row textarea').value"), "書きかけ")
        b.escape()
        self.assertTrue(b.js("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.diffnote-select')"))

    def test_no_thread_is_highlighted_while_lines_are_being_chosen(self):
        self.serve(self.login)
        b = self.b
        # The thread on line 2 shows its range when hovered...
        b.hover(f"{CUR} .diffnote-thread summary")
        self.assertGreaterEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 1)
        # ...but not once a choice is started, nor while dragging over it.
        x, y = b.press(self.gutter("new", 6))
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 0)
        for n in (5, 4, 3, 2):
            x, y = b.center(self.gutter("new", n))
            b.cdp.mouse("mouseMoved", x, y, 1)
            self.assertEqual(b.js("document.querySelectorAll('.diffnote-range').length"), 0, n)
            self.assertEqual(b.js("document.querySelectorAll('.diffnote-thread.diffnote-hover').length"), 0, n)
        b.release(self.gutter("new", 2))
        self.assertEqual(b.js("document.querySelectorAll('.diffnote-select-first').length"), 1)

    def test_a_failed_send_keeps_the_box_and_the_words(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 6))
        b.js("document.querySelector('.diffnote-composer-row textarea').value='止まった後'")
        self.server.shut_down(b)
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!!document.querySelector('.diffnote-compose .diffnote-error')"))
        self.assertEqual(b.js("document.querySelector('.diffnote-composer-row textarea').value"), "止まった後")
        self.assertTrue(b.js("document.querySelector('.diffnote-compose').style.display !== 'none'"))

    def test_the_other_revision_is_drawn_again_when_opened_with_the_line_followed(self):
        self.serve(self.calc)
        b = self.b
        line = f"{CUR} table[data-diffnote-file='calc.py'] tr[data-diffnote-new='14'] .diffnote-line__gutter-new"
        b.click_at(line)
        self.submit_box("mul の戻り値を確認")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(b.js("document.getElementById('rev-0').hasAttribute('data-stale')"))
        self.assertFalse(b.js("document.getElementById('rev-0').textContent.includes('mul の戻り値を確認')"))
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("!document.getElementById('rev-0').hasAttribute('data-stale')"))
        where = b.js("""(function(){var t=Array.from(document.querySelectorAll('#rev-0 .diffnote-thread')).find(function(t){return t.textContent.includes('mul の戻り値を確認')}); return t.querySelector('.diffnote-thread__where').textContent})()""")
        self.assertEqual(where, "calc.py:12", "line 14 of the new revision is line 12 of the old one")
        self.assertGreater(b.js("document.querySelectorAll('#rev-0 .diffnote-reply').length"), 0, "still interactive")


class ThreadsOnFilesAndTheReview(ServedCase):
    def test_a_review_wide_thread_is_added_to_its_place(self):
        self.serve(self.calc)
        b = self.b
        before = b.js(f"document.querySelectorAll('{CUR} [data-diffnote-global] .diffnote-thread').length")
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__where').textContent"), "レビュー全体へのコメント")
        self.type_and_send(".diffnote-compose", "全体の方針について")
        self.assertTrue(b.wait(f"document.querySelectorAll('{CUR} [data-diffnote-global] .diffnote-thread').length === {before + 1} && !document.querySelector('.diffnote-compose-wrap')"))
        self.assertTrue(self.same_page())
        self.assertIn("全体の方針について", show(self.review))

    def test_a_file_thread_is_added_to_the_file_and_the_box_can_be_closed_with_escape(self):
        self.serve(self.calc)
        b = self.b
        b.click(f"{CUR} section.diffnote-file[data-diffnote-file='calc.py'] [data-diffnote-add=file]")
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__where').textContent"), "calc.py へのコメント")
        b.escape()
        self.assertFalse(b.js("!!document.querySelector('.diffnote-compose-wrap')"))
        b.click(f"{CUR} section.diffnote-file[data-diffnote-file='calc.py'] [data-diffnote-add=file]")
        self.type_and_send(".diffnote-compose", "ファイル全体について")
        cards = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py'] [data-diffnote-cards] .diffnote-thread"
        self.assertTrue(b.wait(f"document.querySelectorAll({json.dumps(cards)}).length === 1"))
        self.assertEqual(b.js(f"document.querySelector({json.dumps(cards + ' .diffnote-thread__where')}).textContent"), "calc.py")
        self.assertTrue(any("ファイル全体: calc.py" in l for l in show(self.review).splitlines()))


if __name__ == "__main__":
    unittest.main()
