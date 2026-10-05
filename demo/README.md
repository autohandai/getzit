# Demo recordings

The two animations on the docs home page, reproducible.

| File | Makes |
|---|---|
| `setup.sh` | a temporary repository whose README.md has ten empty sections |
| `edit.sh`, `ten-users.sh` | ten users editing it at once, each in a Zit workspace, two of them on the same section |
| `accept-all.sh` | accepts every change, one at a time |
| `zit-in-action.tape` | the terminal recording (`vhs demo/zit-in-action.tape`) |
| `web-scene.sh`, `web-walkthrough.mjs` | the web view walkthrough (puppeteer-core with Chrome) |
| `render.sh` | video to animated WebP (`render.sh demo/out/zit-in-action.mp4 docs/images/zit-in-action.webp`) |

The ten users are scripted: their edits and reasons are fixed in `ten-users.sh`. Everything else is Zit's real output.
