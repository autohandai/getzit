// The repository's star count, for the header link. Read once per build from GitHub; undefined,
// with a warning, when GitHub cannot be reached, so the link renders without a count.
// GITHUB_TOKEN, when set, avoids GitHub's anonymous rate limit.
const REPO = "autohandai/getzit";
let cached: Promise<number | undefined> | undefined;

export function githubStars(): Promise<number | undefined> {
  cached ??= fetchStars();
  return cached;
}

async function fetchStars(): Promise<number | undefined> {
  const token = process.env.GITHUB_TOKEN;
  try {
    const response = await fetch(`https://api.github.com/repos/${REPO}`, {
      headers: { Accept: "application/vnd.github+json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    });
    if (response.ok) {
      const { stargazers_count: stars } = (await response.json()) as { stargazers_count?: number };
      if (typeof stars === "number") return stars;
    }
    console.warn(`github-stars: GitHub answered ${response.status}; showing the link without a count`);
  } catch (error) {
    console.warn(`github-stars: GitHub unreachable (${error}); showing the link without a count`);
  }
  return undefined;
}

// 1234 -> "1.2k", as GitHub abbreviates counts.
export function formatStars(stars: number): string {
  if (stars < 1000) return String(stars);
  const k = stars / 1000;
  return `${k < 10 ? k.toFixed(1).replace(/\.0$/, "") : Math.round(k)}k`;
}
