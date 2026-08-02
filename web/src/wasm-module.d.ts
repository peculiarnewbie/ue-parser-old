declare module "*uasset_parser_wasm.js" {
  export default function init(): Promise<void>;
  export function parse(kind: string, filename: string, bytes: Uint8Array, options: string): string;
}
