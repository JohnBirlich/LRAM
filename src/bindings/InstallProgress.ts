export type InstallProgress = { "stage": "downloading", percent: number, } | { "stage": "verifying" } | { "stage": "installing" } | { "stage": "done" } | { "stage": "error", message: string, };
