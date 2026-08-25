# anti-slop-swift

[![skills.sh](https://skills.sh/badge/sawfwair/anti-slop-swift)](https://skills.sh/sawfwair/anti-slop-swift)

Opinionated SwiftSyntax rules that reject low-evidence and low-signal Swift patterns. A Swift port of [dmmulroy/anti-slop](https://github.com/dmmulroy/anti-slop), with additional Swift-specific rules.

19 rules, all AST-level (no regex), built on [swift-syntax](https://github.com/swiftlang/swift-syntax). Requires a Swift 6.0+ toolchain.

```console
$ anti-slop Sources
Sources/Payments/RefundService.swift:42:20: error: [no-force-unwrap] Force unwrap crashes on the first unexpected nil. Add a // SAFETY: comment stating why the optional is known non-nil, or unwrap with `guard let`.
Sources/Payments/RefundService.swift:58:9: error: [no-swallowed-errors] Empty catch block swallows the error entirely; failures become invisible. Bind the error, log it, recover deliberately, or rethrow.

[anti-slop] 2 violations across 14 files (19 rules).
```

This project is meant to be vendored, not treated as a fixed dependency. Copy the rules into your repository, read them, and change them to match your team's standards. After that, the vendored files are yours to maintain and make your own.

## Install with an agent skill

```bash
npx skills add sawfwair/anti-slop-swift --skill install-anti-slop-swift
```

Then ask your coding agent to install or configure anti-slop-swift in the current repository. The skill copies the package to `tools/anti-slop-swift/`, establishes a lint baseline, and wires the linter into CI. To inspect available skills first:

```bash
npx skills add sawfwair/anti-slop-swift --list
```

## Manual installation
Copy the package into your repository, for example at `tools/anti-slop-swift/`, then run:

```bash
cd tools/anti-slop-swift
swift run anti-slop ../../Sources
```

Exit code is `0` when clean and `1` when any rule fires. Wire that into CI, or import `AntiSlopCore` from your own tooling:

```swift
import AntiSlopCore

let violations = AntiSlop.lint(
    source: sourceText,
    fileName: path,
    rules: SlopRule.allRules
)
```

Skip rules for a one-off run:

```bash
swift run anti-slop --disable=no-force-unwrap,no-key-value-coding Sources
```

For standing decisions, prefer a committed `.anti-slop.json` (see [Disabling rules](#disabling-rules)). List everything the package ships with:

```bash
swift run anti-slop --list-rules
```

### Linting a repo that vendors this tool

Exclude the vendored copy from its own lint run by scoping the paths you pass (lint `Sources`, not `tools/`). If you also lint this package's tests — where rule names like `NoShapeInSymbolNamesRule` legitimately appear — disable the affected rule for that invocation:

```bash
swift run anti-slop Sources Tests --disable=no-shape-in-symbol-names
```

This is exactly what this repository's CI does.

### Disabling rules

The primary mechanism is a committed `.anti-slop.json` at the repository root — discovered by walking up from wherever you run the linter:

```json
{
  "disabled": ["no-shape-in-symbol-names", "no-key-value-coding"]
}
```

Use it for standing team decisions: domain vocabulary conflicts (`shape` in tensor/geometry code), overlaps with existing SwiftLint policy, or rules the team has consciously rejected. CLI `--disable=RULE,RULE` flags are additive on top of the config for one-off experiments; `--config=FILE` points at an explicit path when needed.

### Rule selection guidance

- `no-shape-in-symbol-names` is upstream parity, but "shape" is core domain vocabulary in tensor/matrix/geometry code. If your domain owns the word, disable it in your config — that is what vendoring is for.
- Running alongside SwiftLint works fine: this tool is AST-based and complements SwiftLint's style rules. Expect overlap only with force-cast/force-try style rules (`swiftlint_rules: force_cast`, `force_try`) — pick one owner per rule to avoid double-reporting.

## Rules

### Generic rules

- `no-chained-type-casts` — rejects nested type casts (`x as! A as! B`) that fabricate evidence. Port of upstream `no-chained-type-assertions`.
- `require-safety-comment-for-forced-cast` — requires every `as!` to document its checked invariant in a preceding `// SAFETY:` comment. Port of upstream `require-safety-comment-for-type-assertion`.
- `no-force-try` — requires every `try!` to document its checked invariant in a preceding `// SAFETY:` comment. Swift-specific.
- `no-force-unwrap` — requires every force unwrap (`x!`) to document why the optional is known non-nil in a preceding `// SAFETY:` comment. Swift-specific.
- `no-fatal-error` — requires every `fatalError`/`preconditionFailure` to document its invariant in a preceding `// SAFETY:` comment. Swift-specific.
- `no-implicitly-unwrapped-optionals` — rejects implicitly unwrapped optionals (`Int!`), which are deferred force unwraps; `@IBOutlet`/`@IBInspectable` declarations are exempt. Swift-specific.
- `no-as-any-cast` — requires casts to `Any`/`AnyObject` to justify discarding type information in a preceding `// SAFETY:` comment. Spirit of upstream `no-known-value-widening`.
- `no-swallowed-errors` — rejects empty `catch` blocks that discard the error without logging, recovering, or rethrowing. Swift-specific.
- `no-any-parameters` — rejects `Any` and `AnyObject` function inputs. `any Error` (optionally optional) is the accepted error-channel convention and is exempt. Port of upstream `no-object-parameters`.
- `no-any-returns` — rejects function contracts that return `Any` or `AnyObject`. Port of upstream `no-unknown-returns`.
- `no-any-typealiases` — rejects aliases that merely conceal `Any`: `typealias Payload = Any`. Port of upstream `no-unknown-type-aliases`.
- `no-any-dictionary-value` — rejects dictionary value contracts based on `Any`/`AnyObject`, such as `[String: Any]`. Port of upstream `no-unsafe-dictionary-type`.
- `no-known-value-widening` — rejects explicit broad annotations that discard known evidence: `let payload: Any = ["a": 1]`, or `let handlers: [String: Handler] = ["start": h]` erasing known keys. Direct port.
- `no-widen-then-assert` — rejects bindings that widen a known value to `Any` and later cast it back (`let stored: Any = load(); let u = stored as! User`). Port of upstream `no-widen-then-assert`.
- `no-shape-in-symbol-names` — rejects the case-insensitive substring "shape" in declaration names. Direct port.
- `no-key-value-coding` — rejects `value(forKey:)`, `value(forKeyPath:)`, `setValue(_:forKey:)`, and `perform(_:)` in favor of typed property access or boundary parsing. Port of upstream `no-reflect-get`.
- `no-runtime-type-sniffing` — rejects `String(describing: type(of: x))` branching on dynamic type; model variants in the type system or decode at the boundary. Port of upstream `no-runtime-typeof`.
- `no-bool-literal-comparisons` — rejects `flag == true`, `flag != false`, and `cond ? true : false`; use the condition directly. Swift-specific.
- `no-hardcoded-secrets` — rejects credentials assigned string literals; read them from the environment or a secrets store. Swift-specific.

### The SAFETY convention

Force casts, force tries, and force unwraps are sometimes genuinely necessary. The escape hatch is always the same: state the invariant you checked by hand.

```swift
// SAFETY: parseUserID validated the string before branding it.
let userID = Int(raw) as! UserID
```

A comment counts only when it sits in the contiguous comment block directly above the offending line; a blank line breaks the chain.

## Violation examples

### `no-chained-type-casts`

```swift
let user = payload as! NSObject as! User
```

### `require-safety-comment-for-forced-cast`

```swift
let userID = value as! UserID
```

Add a specific justification immediately before a necessary cast:

```swift
// SAFETY: parseUserID validated the identifier before branding it.
let userID = value as! UserID
```

### `no-force-try`

```swift
let config = try! JSONDecoder().decode(Config.self, from: bundledData)
```

### `no-force-unwrap`

```swift
let first = results.first!
```

### `no-fatal-error`

```swift
fatalError("unreachable")
```

### `no-implicitly-unwrapped-optionals`

```swift
var cache: [String: Data]!
```

### `no-as-any-cast`

```swift
let boxed = value as Any
```

### `no-swallowed-errors`

```swift
do {
    try run()
} catch {}
```

### `no-bool-literal-comparisons`

```swift
let enabled = cond ? true : false
```

Binary comparisons like `flag == true` are deliberately allowed: on an optional Bool they are meaningful, and operand types are invisible to a syntax-level linter.

### `no-hardcoded-secrets`

```swift
let apiKey = "sk-live-abc123"
```

### `no-any-parameters`

```swift
func handle(input: Any) {}
```

### `no-any-returns`

```swift
func loadUser() -> Any {}
```

### `no-any-typealiases`

```swift
typealias ExternalValue = Any
```

### `no-any-dictionary-value`

```swift
let metadata: [String: Any] = [:]
```

### `no-shape-in-symbol-names`

```swift
struct UserShape {
    let id: String
}
```

### `no-known-value-widening`

```swift
let payload: Any = ["a": 1]
```

Annotated dictionaries with literal keys (`let headers: [String: String] = ["Content-Type": "..."]`) are idiomatic Swift and deliberately not flagged.

### `no-widen-then-assert`

```swift
let stored: Any = loadUser()
let user = stored as! User
```

### `no-key-value-coding`

```swift
let name = owner.value(forKey: "name")
```

### `no-runtime-type-sniffing`

```swift
let kind = String(describing: type(of: value))
```

## Parity with upstream

| upstream rule | status here |
| --- | --- |
| `no-chained-type-assertions` | ✅ `no-chained-type-casts` |
| `no-conditional-empty-object-spread` | ❌ no Swift spread syntax |
| `no-known-value-widening` | ✅ direct port (call expressions not treated as evidence) |
| `no-module-mocking` | ❌ no Vitest/Jest-style module mocking in Swift |
| `no-object-parameters` | ✅ `no-any-parameters` |
| `no-reflect-apply` | ❌ no dynamic apply in Swift |
| `no-reflect-get` | ✅ `no-key-value-coding`; `perform` narrowed to `#selector(...)` arguments so typed `perform([requests])` APIs do not false-positive |
| `no-runtime-typeof` | ◑ `no-runtime-type-sniffing`: string sniffing and `type(of:) == T.self` comparisons are rejected; plain `is`/`as?` checks are allowed because they are idiomatic error and protocol handling in Swift |
| `no-shape-in-symbol-names` | ✅ direct port |
| `no-unknown-parameters` | ✅ `no-any-parameters` (`any Error` exempt, mirroring the `cause` convention) |
| `no-unknown-returns` | ✅ `no-any-returns` |
| `no-unknown-type-aliases` | ✅ `no-any-typealiases` |
| `no-unsafe-dictionary-type` | ◑ `no-any-dictionary-value`: checks `Any`/`AnyObject` values directly; does not resolve aliases like upstream's type environment |
| `no-widen-then-assert` | ✅ ported; source-order scoped instead of function-boundary scoped |
| `require-safety-comment-for-type-assertion` | ✅ `require-safety-comment-for-forced-cast` |
| Effect group | ❌ no Effect ecosystem; Swift dependency seams differ |

Everything marked ✅ is a semantic port read from upstream's implementation, not just its README. Everything else is either impossible in Swift or deliberately narrower, as noted.

## Divergence from upstream

Swift has no spread syntax, module mocking, or Effect ecosystem, so `no-conditional-empty-object-spread`, `no-module-mocking`, and the Effect rules have no port. `no-reflect-apply` has no Swift analog worth a rule. In exchange, Swift's crash-on-unexpected-value operators (`try!`, `x!`, IUOs, `fatalError`) get the same SAFETY-comment treatment upstream applies to type assertions.

## Development

```bash
swift build
swift test
.build/debug/anti-slop Sources Tests
node scripts/sync-skill-assets.mjs --check
```

`Sources/AntiSlopCore/` is canonical. After changing production source, run `node scripts/sync-skill-assets.mjs` so the install skill's bundled assets stay identical; CI enforces this. See `AGENTS.md` and `CONTRIBUTING.md` for contribution guidance.

## Acknowledgments

anti-slop-swift is a port of [dmmulroy/anti-slop](https://github.com/dmmulroy/anti-slop) by [Dillon Mulroy](https://github.com/dmmulroy). The philosophy, the rule set, the vendored-not-dependency distribution model, and the `// SAFETY:` escape-hatch convention all originate there; the generic rules are semantic ports of upstream's Oxlint implementations, and the parity table above maps every rule to its origin. The Swift-specific rules (`no-force-try`, `no-force-unwrap`, `no-fatal-error`, `no-implicitly-unwrapped-optionals`, `no-as-any-cast`, `no-swallowed-errors`, `no-bool-literal-comparisons`, `no-hardcoded-secrets`) extend the same ideas to patterns that only exist in Swift.

Licensing follows that split: this project's original Swift code is Copyright (c) 2026 sawfwair / anti-slop-swift contributors, while the ported portions remain subject to Dillon Mulroy's MIT copyright — both notices live in [LICENSE](LICENSE). Thank you, Dillon.

## License

MIT
