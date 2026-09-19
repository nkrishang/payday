// Makes sure the local SDK dependency is compiled before this package builds.
// The harness imports @gum/sdk (file dep), which ships its types from dist/.
import { existsSync, mkdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
// The harness may run from benchmarks/gum-load or its dist/; walk up to the
// repository root by finding the sdk/ directory.
const candidates = [resolve(here, "../sdk/typescript"), resolve(here, "../../sdk/typescript"), resolve(here, "../../../sdk/typescript")];
const sdkDir = candidates.find((candidate) => existsSync(join(candidate, "tsconfig.json")));
if (!sdkDir) {
  console.error(`@gum/sdk sources not found near ${here}; run the harness from the repository.`);
  process.exit(1);
}
const sdkDist = join(sdkDir, "dist/index.js");
if (existsSync(sdkDist)) {
  process.exit(0);
}
mkdirSync(dirname(sdkDist), { recursive: true });
const result = spawnSync("npm", ["run", "-s", "build"], { cwd: sdkDir, stdio: "inherit" });
process.exit(result.status ?? 1);
