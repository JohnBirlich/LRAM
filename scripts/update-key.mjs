// Makes the key pair the built-in updater needs: a P-256 key whose public half goes into
// `PUBLIC_KEY` in src-tauri/src/self_update.rs and whose private half becomes the LRAM_UPDATE_KEY
// secret of the release workflow.
//
//   node scripts/update-key.mjs
//
// With no key in self_update.rs the updater is off (this fork's default: install releases by hand).
// Print the private half once, paste it in GitHub > Settings > Secrets > Actions, and keep no copy
// anywhere else: it is what makes an installer trusted, so anyone holding it can sign one.
import { generateKeyPairSync } from "node:crypto";

const { publicKey, privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });

// X‖Y, the 64 bytes self_update.rs expects, hex: CNG's ECC point without the 0x04 prefix.
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
