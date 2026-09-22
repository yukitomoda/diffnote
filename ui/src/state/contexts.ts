// What the parts of the page are given without being handed it through every
// component between, and the shape of each.
//
// `null` on an exported page means "nothing can be changed": a component that
// is given one of these checks it before offering anything that would write.
import { createContext } from 'preact';
import type { At, LineRef } from '../lib.ts';
import type {
  Answer,
  AttachedData,
  FileData,
  Hunk,
  OpenedFile,
  Placement,
  RevisionData,
  Settings,
  Side,
  ThreadData,
  ViewModel,
} from '../model.ts';

/**
 * What the server answers a change with. Every one of them says whether it
 * worked; what else it carries depends on the change (the whole model, or only
 * what the page needs to put its own copy right).
 */
export type ChangeAnswer = Answer<{
  model?: ViewModel;
  /** The stamp the review had before the change, and the thread as it is now. */
  before?: string;
  thread_data?: ThreadData;
  /** Whether the target has something new to take in. */
  pending?: boolean;
  stamp?: string;
  editable?: string[];
  changed?: string[];
  /** How long the log is now, and how much this change added to it. */
  events?: number;
  added?: number;
  /** What a pull did, in a line. */
  message?: string;
}>;

/** The lines a new thread is about, on one side. */
export interface LineSpan {
  start: number;
  len: number;
}

/**
 * A new thread, as the page asks for it (see `create_thread` in
 * `src/serve.rs`): what it is about, in which revision, and the text. Lines are
 * said as the new side has them; where they are on the old side is worked out
 * from the revision's own diff unless `base` says.
 */
export interface NewThread {
  scope: 'lines' | 'file' | 'global';
  revision: number;
  file?: string;
  head?: LineSpan;
  base?: LineSpan;
  body?: string;
}

/** What the page can do to the review. */
export interface Actions {
  reply(id: string, text: string): Promise<ChangeAnswer>;
  create(request: NewThread): Promise<ChangeAnswer>;
  edit(id: string, text: string): Promise<ChangeAnswer>;
  remove(id: string): Promise<ChangeAnswer>;
  react(id: string, emoji: string): Promise<ChangeAnswer>;
  saveSettings(settings: Partial<Settings>): Promise<ChangeAnswer>;
  removeAttached(attached: AttachedData): Promise<ChangeAnswer>;
  saveUserSettings(author: string): Promise<ChangeAnswer>;
  refresh(): Promise<ChangeAnswer>;
  setResolved(id: string, resolved: boolean): Promise<ChangeAnswer>;
  /** The comments this page may rewrite or take out, and which it has. */
  editable: Set<string>;
  changed: Set<string>;
  /** The name comments are written under. */
  author?: string;
}

/** The lines being chosen, while they are being chosen. */
export interface Selection {
  rev: number;
  path: string;
  /** The side they are chosen on, in a side by side view; else none. */
  side?: Side | null;
  anchor: number;
  to: number;
}

/** A thread being written about a whole file, or the whole review. */
export interface Scope {
  kind: 'file' | 'global';
  rev: number;
  path?: string;
}

/** Choosing lines, and the box the new thread is written in. */
export interface Compose {
  sel: Selection | null;
  selecting: boolean;
  scope: Scope | null;
  draft: string;
  pending: boolean;
  error: string;
  setDraft(text: string): void;
  close(): void;
  begin(rev: number, path: string, idx: number, shift: boolean, side?: Side | null): void;
  /** A row's index, or what the row at that place on a side is. */
  extend(at: number | ((side: Side | null | undefined) => number | null)): void;
  openScope(kind: 'file' | 'global', rev: number, path?: string): void;
  send(request: NewThread): void;
}

/** The files opened to look at, which are in no revision of the review. */
export interface Opened {
  byRev: Record<number, OpenedFile[]>;
  open(rev: number, path: string): Promise<Answer<{ file?: OpenedFile }>>;
  close(rev: number, path: string): void;
  more(rev: number, path: string): Promise<Answer<{ hunk?: Hunk; next?: number | null }>>;
}

/** Where a jump can go, and what a comment's attachments are. */
export interface Links {
  /** Which revision is shown, and how many there are. */
  current: number;
  revisions: number;
  has(path: string): boolean;
  /** The most an attached file may weigh (the review's own rule). */
  limit: number;
  file(id: string, name: string): string;
  image(id: string): string;
  go(ref: LineRef | At): void;
  jump(rev: number, place: At): void;
}

/**
 * A file as the page shows it: one of the revision's, or one opened to look at
 * (which carries how many lines it has and where the next ones start).
 */
export type ShownFile = FileData & {
  opened?: boolean;
  total?: number;
  next?: number | null;
};

/**
 * What every part of one revision is drawn from: the revision itself, where its
 * threads are, and how it is being shown. Made by `Revision` and handed down as
 * `ctx`.
 */
export interface RevisionCtx {
  /** Which revision this is (0-based, as the model has them). */
  rev: number;
  model: ViewModel;
  revision: RevisionData;
  /** The thread ids in the order of the list, where each is, and each thread. */
  order: string[];
  placements: Record<string, Placement>;
  byId: Record<string, ThreadData>;
  hideResolved: boolean;
  ignoreSpace: boolean;
  layout: 'unified' | 'split';
  /** What this revision is being compared against, where that is not the base. */
  compare?: number | null;
}

/**
 * What the lists beside the diff are drawn from. Not the same as `RevisionCtx`:
 * they count the files of the diff (`diffFiles`, without the ones only opened
 * to look at) and know nothing of how the diff is laid out.
 */
export interface ListCtx {
  rev: number;
  model: ViewModel;
  /** The revision, with the files opened to look at among its own. */
  revision: RevisionData;
  diffFiles: ShownFile[];
  order: string[];
  placements: Record<string, Placement>;
  byId: Record<string, ThreadData>;
  hideResolved: boolean;
}

export const ActionsContext = createContext<Actions | null>(null);
export const ComposeContext = createContext<Compose | null>(null);
export const OpenedContext = createContext<Opened | null>(null);
// Given by the page whatever it is (an exported page jumps about like any
// other); the default stands for nothing and is never the one used.
export const LinksContext = createContext<Links>(null as unknown as Links);
