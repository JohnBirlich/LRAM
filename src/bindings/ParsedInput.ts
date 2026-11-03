import type { LaunchTarget } from "./LaunchTarget";

export type ParsedInput = { "kind": "target", target: LaunchTarget, } | { "kind": "jobIdOnly", jobId: string, } | { "kind": "username", name: string, } | { "kind": "invalid", reason: string, } | { "kind": "unsupported", reason: string, };
