export interface ManualPage {
  route: string;
  title: string;
  summary: string;
  group: string;
  source: string;
}

export function normalizeRoute(hash: string, pages: readonly ManualPage[]): string {
  const candidate = hash.startsWith("#") ? hash.slice(1) : hash;
  if (candidate === "/" || pages.some((page) => page.route === candidate)) {
    return candidate || "/";
  }
  return "/";
}

export function searchPages(query: string, pages: readonly ManualPage[]): ManualPage[] {
  const tokens = query
    .trim()
    .toLocaleLowerCase()
    .split(/\s+/u)
    .filter(Boolean);
  if (tokens.length === 0) return [];
  return pages.filter((page) => {
    const haystack = `${page.title} ${page.summary} ${page.group} ${page.source}`.toLocaleLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}
