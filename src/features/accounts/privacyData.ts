import type { AccountSummary, Group } from "../../api";

// Display projections only. Real IDs remain usable for selection and commands.
export const maskAccount = (a: AccountSummary): AccountSummary => ({ ...a, username: "hidden", displayName: "Hidden", alias: "Hidden", notes: "Hidden", color: null });
export const maskGroup = (g: Group): Group => ({ ...g, name: "Hidden", color: "#808080" });
