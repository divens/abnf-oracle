# Provenance

These inputs come from **JSONTestSuite** by Nicolas Seriot, MIT licensed; the licence is
alongside this file as `LICENSE`.

    https://github.com/nst/JSONTestSuite
    commit 1ef36fa01286573e846ac449e8683f8833c5b26a (2024-11-22)
    directory test_parsing/ only

Filenames are kept exactly as upstream, so any case can be traced back. The suite's own
convention is that `y_` must be accepted, `n_` must be rejected, and `i_` is left to the
implementation; the directories here record where each file landed for *this* crate, which is
not always where the prefix suggests. `NOTES.md` says why for every such case.

To refresh: clone the suite at a newer commit, copy `test_parsing/*.json` into the three
directories by the rules in `NOTES.md`, and update the commit above.
