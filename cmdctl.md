I want to build cmdctl, a new subcommand

It will find commands (anything prefixed with $) embedded in comments (using
treesitter), run them, and then insert the response back into the next non-empty
Treesitter node.

This is useful for keeping comments in source code up to date, help text, etc.

Ex:

```py
# $ seq 1 5 | jq -Rs 'split("\n")[:-1] | map(tonumber)'
numbers = [1, 2, 3, 4, 5]
```

```rust
// $ curl -s httpbin.org/uuid | jq -r .uuid
const REQUEST_ID: &str = "ac8d45e6-8d9a-4b6e-9c3a-1234567890ab";
```

```rust
// $ cat colors.txt
enum Color {
    Red,
    Green,
    Blue,
}
```

```js
// $ node --version | cut -d'v' -f2
const MIN_NODE_VERSION = "20.10.0";
```

```py
# $ python3 -c "print(60 * 60 * 24)"
SECONDS_PER_DAY = 86400
```

---

Architecture Decision: Next-Sibling + Replaceable Nodes
Algorithm:

Parse file with treesitter
Find comment nodes containing $ command
Get next sibling node (skip whitespace)
Check if sibling is a "replaceable" node type
If yes → execute command and replace node content
If no → skip (with optional warning)

Trait-Based Language Support
