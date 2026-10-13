// CI helper: gives each build its own version (major.minor from tauri.conf.json,
// patch = workflow run number) and writes a config override for `tauri build`.
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";

const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const [major, minor] = conf.version.split(".");
const run = process.env.GITHUB_RUN_NUMBER ?? "0";
const version = `${major}.${minor}.${run}`;
const repo = process.env.RELEASE_REPO || process.env.GITHUB_REPOSITORY;
const signing = Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY);

const override = {
  version,
  // Updater artifacts must be signed; without the key, build plain installers.
  bundle: { createUpdaterArtifacts: signing },
  plugins: { updater: { endpoints: [`https://github.com/${repo}/releases/latest/download/latest.json`] } },
};
writeFileSync("src-tauri/tauri.ci.json", JSON.stringify(override, null, 2));

if (process.env.GITHUB_OUTPUT) {
  appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\nsigning=${signing}\n`);
}
console.log(`version ${version}, release repo ${repo}, signing ${signing ? "on" : "off (no TAURI_SIGNING_PRIVATE_KEY secret)"}`);
