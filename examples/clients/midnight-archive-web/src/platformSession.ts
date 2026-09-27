export interface PlatformSessionConfiguration {
  readonly csrf: string;
  readonly browserStreamUrl: string;
}

export function readPlatformSession(value: unknown): PlatformSessionConfiguration {
  if (
    typeof value !== "object" || value === null || Array.isArray(value)
    || !("authenticated" in value) || value.authenticated !== true
    || !("csrf" in value) || typeof value.csrf !== "string"
    || !/^[A-Za-z0-9_-]{43,128}$/u.test(value.csrf)
    || !("browser_stream_url" in value) || typeof value.browser_stream_url !== "string"
    || value.browser_stream_url.length > 2_048
  ) throw new Error("platform_session_configuration_unavailable");

  let stream: URL;
  try {
    stream = new URL(value.browser_stream_url);
  } catch {
    throw new Error("platform_session_configuration_unavailable");
  }
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(stream.hostname);
  if (
    stream.href !== value.browser_stream_url
    || stream.pathname !== "/v1/hosted/browser-stream"
    || value.browser_stream_url.includes("?") || value.browser_stream_url.includes("#")
    || stream.username !== "" || stream.password !== ""
    || (stream.protocol !== "wss:" && !(loopback && stream.protocol === "ws:"))
  ) throw new Error("platform_session_configuration_unavailable");
  return { csrf: value.csrf, browserStreamUrl: stream.href };
}
