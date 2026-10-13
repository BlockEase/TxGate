// Interface languages. Locale files are flat `key → template` maps with
// `{param}` placeholders; the engine shares en.json for its own messages
// (risks, Safe alerts, notifications), so both render the same English.

import { createContext, Fragment, useContext, type ReactNode } from "react";
import en from "./locales/en.json";
import es from "./locales/es.json";
import pt from "./locales/pt.json";
import ru from "./locales/ru.json";
import zhCN from "./locales/zh-CN.json";

export type Lang = "en" | "zh-CN" | "es" | "ru" | "pt";

/** Each language named in itself, so it can be found whatever is shown now. */
export const LANGUAGES: { code: Lang; name: string }[] = [
  { code: "en", name: "English" },
  { code: "zh-CN", name: "简体中文" },
  { code: "es", name: "Español" },
  { code: "ru", name: "Русский" },
  { code: "pt", name: "Português" },
];

const DICTS: Record<Lang, Record<string, string>> = { en, "zh-CN": zhCN, es, ru, pt };

export type Params = Record<string, string | number>;

/** A message from the engine: a key, its parameters and texts shown before it. */
export interface Text {
  key: string;
  params?: Record<string, string>;
  prefix?: Text[];
}

export function isLang(v: unknown): v is Lang {
  return typeof v === "string" && v in DICTS;
}

/** The system language, mapped onto the supported ones. */
export function systemLang(): Lang {
  const l = (navigator.language || "en").toLowerCase();
  // Every Chinese locale, Traditional included, reads the Simplified translation.
  if (l.startsWith("zh")) return "zh-CN";
  if (l.startsWith("es")) return "es";
  if (l.startsWith("ru")) return "ru";
  if (l.startsWith("pt")) return "pt";
  return "en";
}

export function translate(lang: Lang, key: string, params?: Params): string {
  const template = DICTS[lang]?.[key] ?? DICTS.en[key] ?? key;
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) => (name in params ? String(params[name]) : m));
}

export interface I18n {
  lang: Lang;
  /** A UI string. */
  t: (key: string, params?: Params) => string;
  /** An engine message; `fallback` is its English text, used by records written before translations existed. */
  tt: (text: Text | null | undefined, fallback: string) => string;
}

export function makeI18n(lang: Lang): I18n {
  const t = (key: string, params?: Params) => translate(lang, key, params);
  const render = (text: Text): string => (text.prefix ?? []).map(render).join("") + t(text.key, text.params);
  return { lang, t, tt: (text, fallback) => (text ? render(text) : fallback) };
}

const Ctx = createContext<I18n>(makeI18n("en"));

// The language of the current render, for helpers outside components. The
// whole tree re-renders when the language changes (it lives in App state).
let current = makeI18n("en");

/** A UI string in the current language. */
export function t(key: string, params?: Params): string {
  return current.t(key, params);
}

/** A UI string whose `{param}` slots hold elements, e.g. a <code> path. */
export function tn(key: string, nodes: Record<string, ReactNode>): ReactNode[] {
  return t(key)
    .split(/(\{\w+\})/)
    .map((part, i) => {
      const name = part.match(/^\{(\w+)\}$/)?.[1];
      return name && name in nodes ? <Fragment key={i}>{nodes[name]}</Fragment> : part;
    });
}

/** An engine message in the current language. */
export function tt(text: Text | null | undefined, fallback: string): string {
  return current.tt(text, fallback);
}

export function I18nProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  if (current.lang !== lang) current = makeI18n(lang);
  return <Ctx.Provider value={current}>{children}</Ctx.Provider>;
}

export function useI18n(): I18n {
  return useContext(Ctx);
}
