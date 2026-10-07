import { defineComponents } from "blume";
import AgentLogos from "./components/AgentLogos.astro";
import OpenInAutohand from "./components/OpenInAutohand.astro";
import ZitDiagram from "./components/ZitDiagram.astro";
import ZitVersion from "./components/ZitVersion.astro";

export default defineComponents({
  layout: {
    PageFooter: OpenInAutohand,
  },
  mdx: {
    AgentLogos,
    ZitDiagram,
    ZitVersion,
  },
});
