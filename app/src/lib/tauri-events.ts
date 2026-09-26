/* Minimal Tauri v2 event listener shim, kept dependency-free.
   Uses __TAURI_INTERNALS__ the same way @tauri-apps/api does under the hood. */

type UnlistenFn = () => void;

interface OnceFlag {
  (payload: unknown): void;
}

export async function listen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__?: {
        invoke: (cmd: string, args?: unknown) => Promise<unknown>;
        transformCallback?: (cb: (e: { payload: unknown }) => void) => number;
      };
    }
  ).__TAURI_INTERNALS__;
  if (!internals) return () => {};

  const transform = internals.transformCallback ?? ((cb: OnceFlag) => cb as unknown as number);
  const handlerId = transform((e) => handler((e as { payload: T }).payload));

  await internals.invoke("plugin:event|listen", {
    event,
    target: { kind: "Any" },
    handler: handlerId,
  });

  return () => {
    void internals.invoke("plugin:event|unlisten", {
      event,
      eventId: handlerId,
    });
  };
}
