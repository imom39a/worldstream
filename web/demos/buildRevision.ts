import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

/** Display identity only; deployment acceptance verifies the full provider identity. */
export function resolveBuildRevision(
  environment: Pick<NodeJS.ProcessEnv, "VERCEL_GIT_COMMIT_SHA"> = process.env,
  readGitRevision: () => string = () => execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: fileURLToPath(new URL("../..", import.meta.url)),
    encoding: "utf8",
  }).trim(),
): string {
  const revision = environment.VERCEL_GIT_COMMIT_SHA ?? readGitRevision();
  if (!/^[0-9a-f]{40}(?:[0-9a-f]{24})?$/u.test(revision)) {
    throw new Error("invalid product build source revision");
  }
  return revision.slice(0, 12);
}
