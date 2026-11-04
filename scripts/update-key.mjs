import { generateKeyPairSync } from "node:crypto";

const { publicKey, privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });

const spki = publicKey.export({ type: "spki", format: "der" });
const xy = spki.subarray(spki.length - 64).toString("hex");
if (xy.length !== 128) throw new Error("unexpected public key encoding");

const pem = privateKey.export({ type: "pkcs8", format: "pem" }).toString();

console.log("1. PUBLIC_KEY in src-tauri/src/self_update.rs (one line, no spaces):\n");
console.log(`const PUBLIC_KEY: &str = "${xy}";\n`);
console.log("2. The LRAM_UPDATE_KEY secret (GitHub > Settings > Secrets and variables > Actions):\n");
console.log(pem);
console.log("Shown only now. Changing this key means already published installers fail their check,");
console.log("so the version that ships the new PUBLIC_KEY must be installed by hand first.");
