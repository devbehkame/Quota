/** Windows gets the Credential Manager, the docked taskbar widget and self-updates. */
export const isWindows = navigator.userAgent.includes("Windows");

/** Where API keys are kept, for messages. */
export const keyStore = isWindows ? "Windows Credential Manager" : "your system keyring";
