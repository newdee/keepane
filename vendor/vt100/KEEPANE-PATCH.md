# vt100 0.16.2 with a keepane patch

Copied verbatim from crates.io (`vt100 = "0.16.2"`, MIT licence, see LICENSE)
and wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.

One change, in `src/row.rs`:

- `Row::resize` clears a wide character whose first half ends up in the new
  last column after a shrink (mirrors what `Row::truncate` already does).
- `Row::clear_wide` bounds-checks the neighbouring cell.

Without it, shrinking a pane (any split or window resize) through a CJK
character and then erasing near the right edge panics with
`index out of bounds: the len is N but the index is N` (`row.rs:89`).
Regression test: `server::pane::tests::shrink_through_wide_char_then_erase_does_not_panic`.

keepane also adds, beside that fix (each marked `(keepane)` in the source):

- `Grid::scrollback_rows` / `Screen::scrollback_rows`: the scrollback's
  length without moving through it.
- `Grid::set_size` scrolls rows a shrink can no longer show into the
  scrollback instead of dropping them.
- `Grid::scrolled` / `Screen::scrolled_total`: how many rows have ever left
  the top of the screen, so `scrolled_total() + row` names a line for good.
  The command marks of `pane-timestamps` and `list-marks` hang on it.
- `Screen::rows_wrapped`: `rows` with each row's wrapped flag, in one walk;
  `rows` plus `row_wrapped` per row starts from the top of the scrollback
  every time, which the history log (a screenful at a time) cannot afford.
- `Grid::scroll_up` keeps a row that leaves the top of the screen in the
  scrollback (and counts it in `scrolled`) when the scroll region starts at
  the top row but ends above the bottom, as xterm does. Upstream keeps rows
  only when no region is set, so an inline interface that pushes its output
  up past a fixed box at the bottom (Codex CLI) left no history at all.
  Regression test: `server::pane::tests::a_region_from_the_top_row_keeps_what_scrolls_off`.

Drop this directory and the `[patch]` entry once upstream has all of it,
which, for the last five, it will not: they are keepane's own.
