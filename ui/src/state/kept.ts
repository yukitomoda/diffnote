// What is kept between visits, where the browser lets us: what the reader
// prefers, and drafts. A browser that refuses (a private window, blocked site
// data) must not stop the page, so every read and write is wrapped.
export function kept(key: string, fallback: string): string {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v;
  } catch {
    return fallback;
  }
}

export function keep(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* not kept */
  }
}
