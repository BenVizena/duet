// Thin wrappers over the Tauri globals (withGlobalTauri is on, so no bundler is needed).
const T = window.__TAURI__;

export const invoke = (cmd, args) => T.core.invoke(cmd, args);
export const listen = (event, cb) => T.event.listen(event, cb);
export const fileSrc = (path) => T.core.convertFileSrc(path);
