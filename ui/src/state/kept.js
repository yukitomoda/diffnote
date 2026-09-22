// What is kept between visits, where the browser lets us.

// What is kept between visits, where the browser lets us.
export function kept(key, fallback) {
  try {
    var v = localStorage.getItem(key);
    return v === null ? fallback : v;
  } catch (e) {
    return fallback;
  }
}

export function keep(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch (e) {
    /* not kept */
  }
}
