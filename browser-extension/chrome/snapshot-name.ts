const MAX_NAME_CHARS = 256;

export type SnapshotNameSources = {
  ariaLabel?: unknown;
  alt?: unknown;
  title?: unknown;
  placeholder?: unknown;
  innerText?: unknown;
};

function boundedText(value: unknown): string {
  return String(value ?? "")
    .replace(/\s+/gu, " ")
    .trim()
    .slice(0, MAX_NAME_CHARS);
}

/**
 * Build a public semantic label without consulting a form control's current
 * value. Current values can contain passwords, API keys, authentication codes,
 * or other user data and are never snapshot naming metadata.
 */
export function snapshotElementName(sources: SnapshotNameSources): string {
  return boundedText(
    sources.ariaLabel ||
      sources.alt ||
      sources.title ||
      sources.placeholder ||
      sources.innerText,
  );
}
