/**
 * Hands the browser a file whose bytes were fetched with the session token.
 * The merchant routes need `Authorization`, which a plain link cannot carry,
 * so the invoice PDF and the proof are fetched by the SDK and saved from
 * memory. Nothing here touches storage.
 */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // Revoking synchronously cancels the download in some browsers.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

export function saveJson(value: unknown, filename: string): void {
  saveBlob(new Blob([JSON.stringify(value, null, 2)], { type: "application/json" }), filename);
}
