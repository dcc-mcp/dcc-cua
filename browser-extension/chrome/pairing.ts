import type { Pairing } from "./protocol.ts";

export interface PairingStore {
  read(): Promise<unknown>;
  write(pairing: Pairing | null): Promise<void>;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function isPairing(value: unknown): value is Pairing {
  return (
    isObject(value) &&
    typeof value.session_nonce === "string" &&
    Number.isSafeInteger(value.tab_id) &&
    Number.isSafeInteger(value.window_id) &&
    typeof value.origin === "string" &&
    typeof value.document_id === "string"
  );
}

function pairingMismatch(): Error & { code: string } {
  return Object.assign(new Error("request does not match the explicitly paired tab"), {
    code: "pairing_mismatch",
  });
}

export class PairingLifecycle {
  private readonly store: PairingStore;
  private cachedPairing: Pairing | null = null;
  private cacheLoaded = false;
  private cacheGeneration = 0;
  private loadPromise: Promise<Pairing | null> | null = null;

  constructor(store: PairingStore) {
    this.store = store;
  }

  async load(): Promise<Pairing | null> {
    if (this.cacheLoaded) return this.cachedPairing;
    if (this.loadPromise !== null) return this.loadPromise;

    const generation = this.cacheGeneration;
    const loadPromise = this.readAndValidate().then((pairing) => {
      if (this.cacheGeneration !== generation) {
        if (!this.cacheLoaded) this.loadPromise = null;
        return this.load();
      }
      this.cachedPairing = pairing;
      this.cacheLoaded = true;
      return pairing;
    });
    let trackedPromise: Promise<Pairing | null>;
    trackedPromise = loadPromise.finally(() => {
      if (this.loadPromise === trackedPromise) this.loadPromise = null;
    });
    this.loadPromise = trackedPromise;
    return trackedPromise;
  }

  async save(pairing: Pairing): Promise<void> {
    await this.store.write(pairing);
    this.cachedPairing = pairing;
    this.cacheLoaded = true;
    this.cacheGeneration += 1;
  }

  async clear(): Promise<void> {
    await this.store.write(null);
    this.cachedPairing = null;
    this.cacheLoaded = true;
    this.cacheGeneration += 1;
  }

  /**
   * Applies a browser storage change without an extra read. Malformed values
   * stay uncached so the next load can remove them and fail closed.
   */
  updateFromStorage(value: unknown): void {
    this.cacheGeneration += 1;
    if (value == null) {
      this.cachedPairing = null;
      this.cacheLoaded = true;
      return;
    }
    if (isPairing(value)) {
      this.cachedPairing = value;
      this.cacheLoaded = true;
      return;
    }
    this.cachedPairing = null;
    this.cacheLoaded = false;
  }

  async requireMatch(request: {
    session_nonce: string;
    tab_id: number;
  }): Promise<Pairing> {
    const pairing = await this.load();
    if (
      pairing === null ||
      pairing.session_nonce !== request.session_nonce ||
      pairing.tab_id !== request.tab_id
    ) {
      throw pairingMismatch();
    }
    return pairing;
  }

  async clearIfTab(tabId: number): Promise<boolean> {
    const pairing = await this.load();
    if (pairing?.tab_id !== tabId) return false;
    await this.clear();
    return true;
  }

  private async readAndValidate(): Promise<Pairing | null> {
    const value = await this.store.read();
    if (value == null) return null;
    if (!isPairing(value)) {
      await this.store.write(null);
      throw new Error("stored browser pairing is malformed");
    }
    return value;
  }
}
