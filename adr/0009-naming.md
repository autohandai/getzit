# ADR 9: Naming

The architecture is CPSG, the binary is Zit, the product name is Zit.

**Status:** accepted (supersedes the earlier product name)

- **EvoGraph** — the existing research umbrella. Unchanged.
- **CPSG, Causal Program State Graph** — the architecture implemented here. Not an established industry term; it is this project's name for it.
- **`zit`** — the crate and the binary.
- **Zit** — the product name: zillion-scale operations in git. It states the goal, not a measurement; the measured scale is in [Benchmarks](../docs/benchmarks.mdx). It appears only in documentation; renaming touches no code.

The name comes from MathWorld's entry for [zillion](https://mathworld.wolfram.com/Zillion.html): "a generic word for a very large number. The term has no well-defined mathematical meaning." That suits a goal with no fixed number: as many people and agents changing one repository as the work needs. (Conway and Guy, 1996, did give it one: the *n*th zillion is 10<sup>3n+3</sup> in the American system.)
