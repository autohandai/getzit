#!/usr/bin/env bash
# A temporary repository with a README of ten sections; Zit initialised.
set -euo pipefail
DEMO=${DEMO:-$(mktemp -d)/demo}
mkdir -p "$DEMO" && cd "$DEMO"
git init -q -b main
cat > README.md <<'MD'
# Shop

A tiny shop.

## Install

TODO

## Usage

TODO

## Configuration

TODO

## Prices

TODO

## Taxes

TODO

## Discounts

TODO

## Testing

TODO

## Deployment

TODO

## Security

TODO

## Contributing

TODO
MD
git add . && git -c user.name=demo -c user.email=demo@example.com commit -qm "Shop"
zit init >/dev/null
echo "$DEMO"
