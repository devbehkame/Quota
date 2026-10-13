// CI helper: writes latest.json, the manifest the app's updater reads.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const version = process.env.VERSION;
const repo = process.env.RELEASE_REPO || process.env.GITHUB_REPOSITORY;
const dir = "target/release/bundle/nsis";
const exe = readdirSync(dir).find((f) => f.endsWith("-setup.exe"));
if (!version || !exe) throw new Error(`missing version (${version}) or installer in ${dir}`);

const signature = readFileSync(join(dir, `${exe}.sig`), "utf8").trim();
const url = `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(exe)}`;
const entry = { signature, url };
const manifest = {
  version,
  notes: (process.env.NOTES ?? "").split("\n")[0],
  pub_date: new Date().toISOString(),
  platforms: { "windows-x86_64": entry, "windows-x86_64-nsis": entry },
};
writeFileSync(join(dir, "latest.json"), JSON.stringify(manifest, null, 2));
console.log(`latest.json -> ${url}`);
