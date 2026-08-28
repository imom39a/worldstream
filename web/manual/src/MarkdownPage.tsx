import MarkdownIt from "markdown-it";
import { useMemo, type MouseEvent } from "react";

import type { ManualPage } from "./manual";

interface Heading {
  id: string;
  level: number;
  text: string;
}

function slugify(text: string): string {
  return text
    .toLocaleLowerCase()
    .replace(/<[^>]+>/gu, "")
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/(^-|-$)/gu, "");
}

function createRenderer() {
  const markdown = new MarkdownIt({ html: false, linkify: true, typographer: true });
  const slugs = new Map<string, number>();

  markdown.renderer.rules.heading_open = (tokens, index) => {
    const tag = tokens[index].tag;
    const text = tokens[index + 1]?.content ?? "section";
    const base = slugify(text) || "section";
    const count = slugs.get(base) ?? 0;
    slugs.set(base, count + 1);
    const id = count === 0 ? base : `${base}-${count + 1}`;
    return `<${tag} id="${markdown.utils.escapeHtml(id)}">`;
  };

  const defaultLinkOpen = markdown.renderer.rules.link_open ?? ((tokens, index, options, _environment, self) => self.renderToken(tokens, index, options));
  markdown.renderer.rules.link_open = (tokens, index, options, environment, self) => {
    const hrefIndex = tokens[index].attrIndex("href");
    const href = hrefIndex >= 0 ? tokens[index].attrs?.[hrefIndex]?.[1] : undefined;
    if (href?.startsWith("http://") || href?.startsWith("https://")) {
      tokens[index].attrSet("target", "_blank");
      tokens[index].attrSet("rel", "noreferrer noopener");
    }
    return defaultLinkOpen(tokens, index, options, environment, self);
  };

  const defaultFence = markdown.renderer.rules.fence;
  markdown.renderer.rules.fence = (tokens, index, options, environment, self) => {
    const rendered = defaultFence
      ? defaultFence(tokens, index, options, environment, self)
      : self.renderToken(tokens, index, options);
    return `<div class="code-block"><button type="button" class="copy-code" aria-label="Copy code">Copy</button>${rendered}</div>`;
  };

  return markdown;
}

function extractHeadings(source: string): Heading[] {
  const counts = new Map<string, number>();
  return source
    .split("\n")
    .flatMap((line) => {
      const match = /^(#{2,3})\s+(.+)$/u.exec(line);
      if (!match) return [];
      const text = match[2].replace(/[`*_]/gu, "").trim();
      const base = slugify(text) || "section";
      const count = counts.get(base) ?? 0;
      counts.set(base, count + 1);
      return [{ id: count === 0 ? base : `${base}-${count + 1}`, level: match[1].length, text }];
    });
}

export function MarkdownPage({ page }: { page: ManualPage }) {
  const html = useMemo(() => createRenderer().render(page.source), [page.source]);
  const headings = useMemo(() => extractHeadings(page.source), [page.source]);

  async function copyCode(event: MouseEvent<HTMLElement>) {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".copy-code");
    if (!button) return;
    const text = button.parentElement?.querySelector("code")?.textContent;
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "Copied";
      window.setTimeout(() => { button.textContent = "Copy"; }, 1400);
    } catch {
      button.textContent = "Select text";
    }
  }

  return (
    <div className="article-layout">
      <article className="doc" onClick={(event) => void copyCode(event)} dangerouslySetInnerHTML={{ __html: html }} />
      {headings.length > 2 && (
        <aside className="on-this-page" aria-label="On this page">
          <p>On this page</p>
          {headings.map((heading) => (
            <button
              className={heading.level === 3 ? "nested" : ""}
              key={heading.id}
              type="button"
              onClick={() => document.getElementById(heading.id)?.scrollIntoView({ behavior: "smooth" })}
            >
              {heading.text}
            </button>
          ))}
        </aside>
      )}
    </div>
  );
}
