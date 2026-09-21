"""`diffnote serve`'s page: replies,
resolving, the version check, shutting down."""
import unittest

import json
import time

from harness import BrowserCase, Served, entries, make_calc_review, make_gaps_review, make_login_review, show
import os
import shutil

CUR = ".diffnote-revision.is-current"


class ServedCase(BrowserCase):
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
        self.b.open(self.server.url)
        self.b.js("localStorage.setItem('diffnote-hide-resolved','0'); localStorage.setItem('diffnote-layout','unified')")
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


class Replies(ServedCase):
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
        written = entries(self.review)
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 2 件)")
        self.assertTrue(self.same_page())
        deadline = time.time() + 8
        while entries(self.review) == written and time.time() < deadline:
            time.sleep(0.05)
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)")
        self.assertIn("再オープン", show(self.review))

    def test_pressing_resolve_twice_quickly_ends_as_the_last_press_says(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        written = entries(self.review)
        # The second press as soon as the page shows the first, before the answer.
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        deadline = time.time() + 8
        while entries(self.review) < written + 2 and time.time() < deadline:
            time.sleep(0.05)
        time.sleep(0.3)
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)", "resolved then reopened: as it was")
        self.assertTrue(b.js(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))

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

    def test_the_export_button_gives_the_page_the_export_command_writes(self):
        self.serve()
        b = self.b
        self.assertEqual(b.js("document.querySelector('[data-diffnote-export]').getAttribute('href')"), "/export")
        # No `download` attribute: with a bare one the page (Preact) made it
        # `download="true"` and the browser saved the file as "true". The
        # name comes from the server's Content-Disposition.
        self.assertFalse(b.js("document.querySelector('[data-diffnote-export]').hasAttribute('download')"))
        # What the link fetches: an attachment named after the bundle, with the
        # comments in it and nothing that talks to the server.
        b.js("fetch('/export',{credentials:'same-origin'}).then(function(r){return r.text().then(function(t){window.__export={disposition:r.headers.get('content-disposition'),text:t}})})")
        self.assertTrue(b.wait("!!window.__export"))
        self.assertIn('filename="r.html"', b.js("window.__export.disposition"))
        self.assertTrue(b.js("window.__export.text.includes('mul の型') && !window.__export.text.includes('D.api = ')"))

    def reply_to(self, card, text, shows=None):
        b = self.b
        b.js(f"var t=document.getElementById({card!r}).querySelector('textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,{text!r}); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        time.sleep(0.1)
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes({(shows or text)!r}) && !document.querySelector('.is-pending')"))

    def test_what_a_comment_says_is_text_and_markdown_never_html_or_script(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        body = '<img src=x onerror="window.__ran=1"> **強調** [悪い](javascript:window.__ran=2) [良い](https://example.com/a) `code`'
        self.reply_to(card, body, shows="良い")
        mine = f"[data-diffnote-comment]:has([data-diffnote-edit]) .diffnote-comment__body"
        self.assertEqual(b.count(f"{mine} img, {mine} script"), 0, "no element from raw HTML")
        self.assertIn('<img src=x onerror="window.__ran=1">', b.text(mine), "it is shown as the text it is")
        self.assertEqual(b.count(f"{mine} strong"), 1)
        self.assertEqual(b.count(f"{mine} code"), 1)
        self.assertEqual(b.count(f"{mine} a"), 1, "only the safe link is a link")
        self.assertEqual(b.js(f"document.querySelector('{mine} a').getAttribute('href')"), "https://example.com/a")
        self.assertIn("悪い", b.text(mine))
        time.sleep(0.3)
        self.assertFalse(b.js("'__ran' in window"))

    def test_only_comments_added_in_this_session_have_edit_and_delete(self):
        self.serve()
        b = self.b
        self.assertEqual(b.count("[data-diffnote-edit], [data-diffnote-delete]"), 0, "what was there before is settled")
        card = self.card("mul の型")
        self.reply_to(card, "あとから書いた返信")
        mine = f"[data-diffnote-comment]:has([data-diffnote-edit])"
        self.assertEqual(b.count(mine), 1)
        self.assertIn("あとから書いた返信", b.text(mine))

    def test_a_comment_of_this_session_can_be_edited(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "書き間違えた")
        b.click("[data-diffnote-edit]")
        self.assertTrue(b.wait_exists("[data-diffnote-edit-form] textarea"))
        self.assertEqual(b.value("[data-diffnote-edit-form] textarea"), "書き間違えた", "the text as written")
        self.write("[data-diffnote-edit-form] textarea", "書き直した")
        b.js("document.querySelector('[data-diffnote-edit-form]').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-edit-form]') && document.body.textContent.includes('書き直した')"))
        self.assertFalse(b.js("document.body.textContent.includes('書き間違えた')"))
        self.assertTrue(self.same_page())
        out = show(self.review)
        self.assertIn("書き直した", out)
        self.assertNotIn("書き間違えた", out)
        # Cancelling leaves it alone.
        b.click("[data-diffnote-edit]")
        self.assertTrue(b.wait_exists("[data-diffnote-edit-form]"))
        self.write("[data-diffnote-edit-form] textarea", "やっぱりやめた")
        b.js("Array.from(document.querySelectorAll('[data-diffnote-edit-form] button')).find(function(x){return x.textContent==='キャンセル'}).click()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-edit-form]')"))
        self.assertIn("書き直した", show(self.review))

    def test_a_reply_of_this_session_can_be_deleted_after_confirming(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "消したい返信")
        b.js("window.confirm=function(){window.__asked=true; return false}")
        b.click("[data-diffnote-delete]")
        self.assertTrue(b.js("window.__asked"))
        self.assertIn("消したい返信", show(self.review), "declined: still there")
        b.js("window.confirm=function(){return true}")
        b.click("[data-diffnote-delete]")
        self.assertTrue(b.wait("!document.body.textContent.includes('消したい返信')"))
        self.assertNotIn("消したい返信", show(self.review))
        self.assertEqual(b.count("[data-diffnote-delete]"), 0)
        self.assertTrue(b.js(f"!!document.getElementById({card!r})"), "the thread stays")

    def test_a_new_thread_can_be_taken_out_again_with_what_it_was_given(self):
        self.serve()
        b = self.b
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.write(".diffnote-compose textarea", "やっぱり要らない全体コメント")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("document.body.textContent.includes('やっぱり要らない全体コメント') && !document.querySelector('.diffnote-compose-wrap')"))
        threads = self.counts()
        b.js("window.confirm=function(){return true}")
        b.click("[data-diffnote-global] [data-diffnote-delete]")
        self.assertTrue(b.wait("!document.body.textContent.includes('やっぱり要らない全体コメント')"))
        self.assertNotEqual(self.counts(), threads)
        self.assertNotIn("やっぱり要らない全体コメント", show(self.review))

    def test_the_shutdown_button_stops_the_server(self):
        self.serve()
        self.b.js("document.querySelector('[data-diffnote-shutdown]').click()")
        self.assertTrue(self.b.wait("document.body.textContent.includes('終了しました')"))


LOGIN = "table[data-diffnote-file='src/auth/login.ts']"


class NewThreadsOnLines(ServedCase):
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


class ThreadsOnFilesAndTheReview(ServedCase):
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


class SideBySideLines(ServedCase):
    def serve_split(self):
        self.serve(self.login)
        self.b.js("localStorage.setItem('diffnote-layout','split')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")
        self.assertTrue(self.b.exists(f"{CUR} {LOGIN}.diffnote-diff--split"))

    def gutter(self, side, n):
        return f"{CUR} {LOGIN} td.diffnote-line__gutter-{side}[data-diffnote-{side}='{n}']"

    def picked(self, side=None):
        cells = "td.is-picked"
        if side:
            cells += f".diffnote-cell--{'removed' if side == 'old' else 'added'}"
        return self.b.count(f"{CUR} {LOGIN} {cells}")

    def test_an_added_line_is_chosen_on_the_new_side_only(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 10))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:10")
        self.assertEqual(self.picked(), 2, "its number and its text")
        self.assertEqual(b.count(f"{CUR} {LOGIN} .diffnote-cell--removed.is-picked"), 0)
        self.assertEqual(b.count(f"{CUR} {LOGIN} .diffnote-composer-row td[colspan='4']"), 1)
        self.write(".diffnote-composer-row textarea", "追加した行について")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(self.same_page())
        self.assertTrue(b.wait_exists(f"{CUR} {LOGIN} .diffnote-thread-row"))
        out = show(self.review)
        self.assertIn("追加した行について", out)
        line = [l for l in out.splitlines() if "login.ts:10" in l]
        self.assertTrue(line and "<-" not in line[0], out)

    def test_a_removed_line_is_chosen_on_the_old_side_only(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("old", 8))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:8")
        self.assertEqual(self.picked("old"), 2)
        self.assertEqual(self.picked("new"), 0)
        self.write(".diffnote-composer-row textarea", "消した行について")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("login.ts:8" in l for l in show(self.review).splitlines()), show(self.review))

    def test_dragging_stays_on_the_side_it_started_on(self):
        self.serve_split()
        b = self.b
        b.drag(self.gutter("new", 9), self.gutter("new", 11))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:9-11")
        self.assertEqual(self.picked("new"), 6, "three lines, number and text")
        self.assertEqual(self.picked("old"), 0)
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked-first"), 2)
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked-last"), 2)
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.is-picked')"))

    def test_an_unchanged_line_is_on_both_sides_and_shift_click_extends_on_the_same_side(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 2))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:2")
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked"), 2)
        b.click_at(self.gutter("new", 4), modifiers=8)
        self.assertTrue(b.wait("document.querySelector('.diffnote-compose__where').textContent==='src/auth/login.ts:2-4'"))
        self.write(".diffnote-composer-row textarea", "書きかけ")
        # Pressing the other side starts a new choice; the draft stays.
        b.click_at(self.gutter("old", 5))
        self.assertTrue(b.wait("document.querySelector('.diffnote-compose__where').textContent!=='src/auth/login.ts:2-4'"))
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked"), 2, "one cell's number and text, on the old side")
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "書きかけ")

    def test_cards_between_are_not_selected_along_with_one_side(self):
        self.serve_split()
        b = self.b
        b.js("document.querySelector(\"td.diffnote-line__gutter-new[data-diffnote-new='3']\").nextElementSibling.setAttribute('data-t','x')")
        b.drag("[data-t=x]", "[data-t=x]")
        # Everything in a card (its text, the reply box, the buttons) can't be selected.
        selectable = b.js("Array.from(document.querySelectorAll('.diffnote-diff--split .diffnote-thread-row, .diffnote-diff--split .diffnote-thread-row *')).filter(function(e){return getComputedStyle(e).userSelect!=='none'}).map(function(e){return e.tagName})")
        self.assertEqual(selectable, [])
        self.assertGreater(b.count(".diffnote-diff--split .diffnote-thread-row textarea"), 0)

    def test_changing_the_layout_lets_go_of_the_choice(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 10))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        b.click("[data-diffnote-layout=unified]")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.is-picked, .diffnote-select')"))


class ExpandLeftOutLines(ServedCase):
    """The served page asks the server for the lines a diff leaves out."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.gaps, _ = make_gaps_review(cls.root)

    def test_the_lines_come_from_the_server_a_few_at_a_time_and_all_at_once(self):
        self.serve(self.gaps)
        b = self.b
        rows = lambda: b.js("Array.from(document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]')).map(function(r){return +r.getAttribute('data-diffnote-new')})")
        marker = lambda n: f"Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){{return r.textContent.includes('{n}')}})[0]"
        before = len(rows())
        b.js(marker(53) + ".querySelector('[data-diffnote-expand=top]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 20}"))
        self.assertTrue(self.same_page())
        # The rest of that place at once.
        b.js(marker(33) + ".querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 53}"))
        got = rows()
        self.assertEqual(got[got.index(23):got.index(77)], list(range(23, 77)))
        self.assertIn("row 50", b.text("table.diffnote-diff"))
        # Looking records nothing.
        self.assertEqual(entries(self.review), 3)

    def test_a_comment_can_be_written_on_a_line_that_was_left_out_and_shown(self):
        self.serve(self.gaps)
        b = self.b
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('53')})[0].querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait("document.querySelectorAll('.diffnote-diff tr[data-diffnote-new=\"50\"]').length === 1"))
        b.click_at(f"{CUR} table[data-diffnote-file='long.txt'] tr[data-diffnote-new='50'] .diffnote-line__gutter-new")
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "long.txt:50")
        self.write(".diffnote-composer-row textarea", "ここも気になります")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertIn("long.txt:50", show(self.review))
        # The thread brought its own context in; what was shown stays shown.
        self.assertTrue(b.wait_exists(f"{CUR} table[data-diffnote-file='long.txt'] .diffnote-thread-row"))
        self.assertTrue(b.exists("tr[data-diffnote-new='30']"))
        self.assertTrue(b.exists("tr[data-diffnote-new='70']"))


if __name__ == "__main__":
    unittest.main()
