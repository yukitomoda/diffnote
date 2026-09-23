// The data the page is drawn from, as Rust writes it: `src/html/viewmodel.rs`
// is where it is worked out and where its form is documented, and these types
// are that form. Nothing here exists at run time -- it is what the page is
// checked against, so that a field renamed in Rust is a mistake here too.
//
// The keys of a row are short because a big diff has many of them (`k` kind,
// `o`/`n` the line numbers, `t` the text, `w` the words that changed).

/** A piece of a line: its text, or its text with the kind that colors it. */
export type Token = string | [kind: string, text: string];

/** `c` unchanged, `a` added, `d` removed. */
export type RowKind = 'c' | 'a' | 'd';

export interface Row {
  k: RowKind;
  /**
   * The line number on the old and the new side; absent where the line is not
   * on that side.
   */
  o?: number;
  n?: number;
  t: Token[];
  /** `[start, end)` in UTF-16 units of the text. */
  w?: [number, number][];
}

export interface Hunk {
  header: string;
  rows: Row[];
}

/** A run of lines the diff doesn't show (the same on both sides). */
export interface Gap {
  /** How many lines, and the old and new number of the first of them. */
  n: number;
  o: number;
  w: number;
  /** The lines themselves, where the page has them (an export carries some). */
  t?: Token[][];
  /** Whether they can be had at all: the text of the file is known. */
  x?: boolean;
}

export type FileStatus = 'modified' | 'added' | 'deleted' | 'renamed' | 'binary' | 'context';

export interface FileData {
  path: string;
  old_path?: string;
  status: FileStatus;
  /** What was done to a binary file, which has no lines to tell it by. */
  change?: 'added' | 'deleted' | 'renamed' | 'modified';
  /**
   * The two versions' digests: a file marked as looked at is taken for a new
   * one when this changes.
   */
  sig?: string;
  hunks: Hunk[];
  /**
   * One before the first hunk, one between each two, one after the last;
   * `null` where nothing is left out.
   */
  gaps?: (Gap | null)[];
}

export type Side = 'new' | 'old';

/** Why the lines a thread is about are not in a revision. */
export type Absence = 'deleted' | 'not-yet' | 'unknown';

/** Where a thread is in one revision. */
export type Placement =
  | {
      kind: 'line';
      file: string;
      side: Side;
      start: number;
      end: number;
      /** The old-side lines a `new` placement also covers (a replaced block). */
      old_range?: [number, number];
      /** Which of the palette's colors is the thread's. */
      color: number;
    }
  | { kind: 'file'; file: string }
  | { kind: 'global' }
  | { kind: 'point'; file: string; before: number; absence: Absence; was: string[] }
  /** The versions needed to place it are not held. */
  | { kind: 'unplaced'; file: string; was: string[] };

export interface RevisionData {
  label: string;
  /** When it was recorded (RFC 3339): the page says it in the reader's time. */
  at: string;
  /** The diff's files, then the ones only threads bring in. */
  files: FileData[];
  placements: Record<string, Placement>;
  /** The thread ids in the order of the thread list. */
  order: string[];
}

// ---- a comment's text -----------------------------------------------------

/**
 * A comment's text as it was parsed (see `src/html/markdown.rs`): a string is
 * text, and everything else is an element with children in `c`. There is no
 * HTML anywhere in it.
 */
export type DocNode = string | DocElement;

export interface DocElement {
  t: string;
  /** The children of an element that has any. */
  c?: DocNode[];
  /** The text of `code` and `pre`. */
  s?: string;
  /** `h`: which level. `ol`: which number it starts at. */
  l?: number;
  start?: number;
  /** `pre`: the language it was marked with. */
  lang?: string;
  /** `a`: where it points, if it is one of the addresses we follow. */
  href?: string;
  /** `table`: how each column is set. */
  al?: (string | null)[];
  /** `image` and `file`: which attachment it shows, and an image's description. */
  id?: string;
  alt?: string;
}

// ---- the review -----------------------------------------------------------

export interface Reaction {
  emoji: string;
  authors: string[];
}

export interface CommentData {
  id: string;
  author: string;
  /** RFC 3339, UTC. */
  at: string;
  doc: DocNode[];
  /** The text as written; the served page has it, to edit it. */
  body?: string;
  /**
   * Taken out: a thread's first comment is kept, with no text, while replies
   * stand on it.
   */
  deleted?: boolean;
  reactions?: Reaction[];
}

export interface ThreadData {
  id: string;
  resolved: boolean;
  /** The first comment, then the replies. */
  comments: CommentData[];
}

/** The review's own settings, as they are saved in it. */
export interface Settings {
  attachment_limit: number;
  title?: string;
  ignore_whitespace?: boolean;
}

/** This machine's settings (`diffnote config`), which apply to every review. */
export interface UserSettings {
  author?: string;
}

/** A number of things, and how many bytes they are. */
export interface Count {
  count: number;
  bytes: number;
}

/** One image or other file attached to a comment. */
export interface AttachedData {
  /** The digest it is stored and referred to by. */
  id: string;
  kind: 'image' | 'file';
  /** An image's media type; other files' types are never read. */
  media_type?: string;
  size: number;
  /** What the file was called where it was attached from; absent if it came
   * without a name, as a pasted screenshot does. */
  name?: string;
}

/** What the bundle holds, for the screens that show it. */
export interface BundleInfo {
  size: number;
  revisions: number;
  images: Count;
  files: Count;
  /** Biggest first. Which comments use one is worked out by the page. */
  attachments: AttachedData[];
}

/** One commit of a revision's trail, as the timeline lists it. */
export interface TimelineCommit {
  id: string;
  short: string;
  author: string;
  /** RFC 3339, in the offset it was written in. */
  at: string;
  subject: string;
  body?: string;
  files?: { path: string; old_path?: string; status: string }[];
}

/**
 * What happened to the review, oldest first: the event log as a reader reads
 * it. A comment says which one it is, not what it says -- its text is in
 * `threads`, where the rest of the page reads it from.
 */
export type TimelineEntry =
  | { kind: 'started'; at: string }
  | { kind: 'revision'; at: string; rev: number; label: string; commits?: TimelineCommit[] }
  | { kind: 'comment'; at: string; author: string; thread: string; reply?: boolean; comment: string }
  | { kind: 'resolved'; at: string; author: string; thread: string }
  | { kind: 'reopened'; at: string; author: string; thread: string };

/** What every revision is compared with. */
export type BaseData = { kind: 'git'; id: string } | { kind: 'files'; at: string };

export interface ViewModel {
  version: number;
  /**
   * A digest of the review's log when this was made: what the page compares to
   * see whether the review has changed under it.
   */
  stamp: string;
  /** Whether the page can change the review (the served one). */
  interactive: boolean;
  title: string | null;
  base: BaseData | null;
  threads: ThreadData[];
  /** Oldest first; the last is shown first. */
  revisions: RevisionData[];
  /** What happened to the review, oldest first. */
  timeline?: TimelineEntry[];
  /** The most an attached file may weigh, in bytes. */
  attachment_limit: number;
  /** Whether lines that differ only in white space start out as unchanged. */
  ignore_whitespace?: boolean;
  /**
   * The pictures and other files the comments show, by id, as `data:`
   * addresses. An exported page carries them; the served one asks for them.
   */
  images?: Record<string, string>;
  attachments?: Record<string, string>;

  // Only the served page has these.

  /** How long the review's log was (the served page keeps count). */
  events?: number;
  /** The comments the page may edit or delete, and which of them it has. */
  editable?: string[];
  changed?: string[];
  /** The name comments are written under. */
  author?: string;
  settings?: Settings;
  bundle?: BundleInfo;
  user_settings?: UserSettings;
  /**
   * Whether the page may ask for what was added to the target since it
   * started.
   */
  refreshable?: boolean;
}

// ---- what the server answers ----------------------------------------------

/**
 * Every answer says whether it worked; a refusal, and a server that can't be
 * reached, say why in `error`.
 */
export interface Refusal {
  ok: false;
  error: string;
}

export type Answer<T> = Refusal | ({ ok: true } & T);

/**
 * The whole model again, after a change: `events` is how long the log is now,
 * and `added` how much this change put in it.
 */
export type ModelAnswer = Answer<{ model: ViewModel; events?: number; added?: number }>;

/**
 * A file opened to look at: its first lines as a hunk of unchanged lines, how
 * many it has, and where the next ones start (`null` at the end).
 */
export interface OpenedFile {
  path: string;
  total: number;
  hunks: Hunk[];
  next: number | null;
}

export type TreeEntry =
  | { kind: 'dir'; path: string; name: string; count: number }
  | { kind: 'file'; path: string; name: string };

export type TreeAnswer = Answer<{
  entries: TreeEntry[];
  /** Said instead of the entries (nothing here), or beside them (a warning). */
  message: string | null;
  note: string | null;
  /** How many more there are than are listed. */
  more: number;
}>;

/** What is attached to a comment, after uploading it. */
export type UploadAnswer = Answer<{ id: string; size: number; bundle_size: number }>;
