import { resolve } from "node:path";
import { createDevelopmentFakeOpenRouter } from "./hosted-fake-openrouter.mjs";
import { controlledArchivePlan } from "./hosted-roster-fixture.mjs";

export function controlledRosterCompletion(request) {
  let invocation;
  try { invocation = JSON.parse(request.messages.at(-1).content); } catch { return null; }
  const proposal = controlledArchivePlan(invocation);
  const provider = request.provider?.only?.[0];
  if (proposal === null || typeof provider !== "string") return null;
  return { provider, content: JSON.stringify(proposal) };
}

export function createRosterFixtureProvider(environment = process.env) {
  return createDevelopmentFakeOpenRouter(environment, { controlledCompletion: controlledRosterCompletion });
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  const { server, port, bind } = createRosterFixtureProvider();
  server.listen(port, bind, () => console.log("Controlled roster fixture provider listening on loopback; no paid provider calls."));
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => server.close());
}
