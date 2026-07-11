import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import { ueParserApiPlugin } from "./vite.parse-plugin.ts";

export default defineConfig({
  plugins: [solid(), ueParserApiPlugin()],
  server: {
    port: 5173,
  },
});
