"""Fail if a Markdown file links to a file in the repository that does not exist."""
import os, re, subprocess, sys

broken = []
for path in subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "*.md"], capture_output=True, text=True).stdout.split():
    if not os.path.exists(path):
        continue
    text = open(path, encoding="utf-8").read()
    text = re.sub(r"```.*?```", "", text, flags=re.S)
    for target in re.findall(r"\]\(([^)\s]+)\)", text):
        if re.match(r"^[a-z]+:|^#|^/", target):
            continue
        rel = target.split("#")[0]
        if rel and not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(path), rel))):
            broken.append(f"{path}: {target}")
print("\n".join(broken) if broken else "all Markdown links resolve")
sys.exit(1 if broken else 0)
