// Small things about the document itself.
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';

/** An id for a path (as the older page made it): letters and digits, else `-`. */
export function htmlId(s: string): string {
  return s.replace(/[^A-Za-z0-9]/g, '-');
}

/**
 * A box that grows and shrinks with what is written in it (wrapped lines too),
 * up to what the style allows; below what `rows` gives it, it doesn't go.
 */
function growTo(el: HTMLElement | null): void {
  if (!el || !el.offsetParent) return;
  el.style.height = 'auto';
  const css = window.getComputedStyle(el);
  const px = (v: string) => parseFloat(v) || 0;
  const height =
    css.boxSizing === 'border-box'
      ? el.scrollHeight + px(css.borderTopWidth) + px(css.borderBottomWidth)
      : el.scrollHeight - px(css.paddingTop) - px(css.paddingBottom);
  el.style.height = height + 'px';
}

export function useAutoGrow(
  ref: { current: HTMLElement | null },
  value: unknown,
  live?: unknown,
): void {
  useLayoutEffect(() => {
    growTo(ref.current);
  }, [value, live]);
  useEffect(() => {
    const on = () => growTo(ref.current);
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  }, []);
}

/** Whether the window is wide enough for two columns of code. */
export function useWide(): boolean {
  const query = '(min-width: 900px)';
  const [wide, setWide] = useState(window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const on = () => setWide(mq.matches);
    if (mq.addEventListener) mq.addEventListener('change', on);
    else mq.addListener(on);
    return () => {
      if (mq.removeEventListener) mq.removeEventListener('change', on);
      else mq.removeListener(on);
    };
  }, []);
  return wide;
}
