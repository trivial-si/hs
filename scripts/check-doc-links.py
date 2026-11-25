"""Check relative Markdown links and HTML image paths without network access."""
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit

root = Path(__file__).resolve().parents[1]
errors = []
for path in root.rglob("*.md"):
    if "target" in path.relative_to(root).parts or ".git" in path.relative_to(root).parts:
        continue
    text = path.read_text()
    links = re.findall(r"\]\(([^)]+)\)", text)
    links += re.findall(r'(?:src|href)="([^"]+)"', text)
    for link in links:
        parsed = urlsplit(link.split(' "')[0])
        if parsed.scheme or parsed.netloc or not parsed.path:
            continue
        if not (path.parent / unquote(parsed.path)).exists():
            errors.append(f"{path.relative_to(root)}: {link}")
if errors:
    raise SystemExit("Missing documentation targets:\n" + "\n".join(errors))
print("Relative documentation links are valid.")
