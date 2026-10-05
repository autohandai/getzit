import { defineComponents } from "blume";
import OpenInAutohand from "./components/OpenInAutohand.astro";

export default defineComponents({
  layout: {
    PageFooter: OpenInAutohand,
  },
});
