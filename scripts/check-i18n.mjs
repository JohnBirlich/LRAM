import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");
const en = JSON.parse(readFileSync(join(root, "src/i18n/locales/en.json"), "utf8"));

function sourceFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return e.name === "locales" ? [] : sourceFiles(path);
    return /\.(ts|tsx)$/.test(e.name) ? [path] : [];
  });
}

const used = new Set();
for (const file of sourceFiles(join(root, "src"))) {
  const code = readFileSync(file, "utf8");
  for (const m of code.matchAll(/\b(?:t|tx)\(\s*"([^"]+)"/g)) used.add(m[1]);
}

const problems = [];
for (const key of used) {
  if (typeof en[key] === "string") continue;
  if (typeof en[`${key}_other`] === "string") continue;
  problems.push(`en.json: no "${key}"`);
}
for (const key of Object.keys(en)) {
  if (key.endsWith("_one") && typeof en[`${key.slice(0, -4)}_other`] !== "string") {
    problems.push(`en.json: "${key}" has no _other form`);
  }
}

if (problems.length) {
  for (const p of problems.slice(0, 200)) console.error(p);
  console.error(`\n${problems.length} string problem(s).`);
  process.exit(1);
}
console.log(`Strings complete: ${used.size} keys used, ${Object.keys(en).length} in en.json.`);
