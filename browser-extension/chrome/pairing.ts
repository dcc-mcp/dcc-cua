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

  constructor(store: PairingStore) {
    this.store = store;
  }

  async load(): Promise<Pairing | null> {
    const value = await this.store.read();
    if (value == null) return null;
    if (!isPairing(value)) {
      await this.store.write(null);
      throw new Error("stored browser pairing is malformed");
    }
    return value;
  }

  async save(pairing: Pairing): Promise<void> {
    await this.store.write(pairing);
  }

  async clear(): Promise<void> {
    await this.store.write(null);
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
}
