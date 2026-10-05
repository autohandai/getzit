#!/usr/bin/env bash
# Ten users change README.md at the same moment, each in a Zit workspace of their own.
# chen and jun both write the Prices section, and disagree.
D=$(dirname "$(readlink -f "$0")")
"$D/edit.sh" ana  Install      'Run `cargo install shop`.'               "New users kept asking how to install it." &
"$D/edit.sh" bo   Usage        'Run `shop --help` to list the commands.' "The usage section was empty." &
"$D/edit.sh" chen Prices       'Prices are integers, in cents.'          "Floats caused rounding bugs at checkout." &
"$D/edit.sh" dev  Taxes        'Tax is 20%, applied after discounts.'    "Matches the tax rule finance confirmed." &
"$D/edit.sh" eli  Discounts    'Discounts apply before tax.'             "Support kept asking about the order." &
"$D/edit.sh" fay  Testing      'Run `cargo test`.'                       "New contributors did not know how to test." &
"$D/edit.sh" gus  Deployment   'Deploy with `shop deploy`.'              "Documents the release script." &
"$D/edit.sh" hana Security     'Report issues to security@example.com.'  "We had no disclosure address." &
"$D/edit.sh" ivo  Contributing 'Open a pull request against main.'       "Explains the workflow to new contributors." &
"$D/edit.sh" jun  Prices       'Prices are in dollars.'                  "I assumed prices were dollars." &
wait
