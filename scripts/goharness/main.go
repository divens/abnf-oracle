// Command goharness exposes pandatix/go-abnf through the same shape of command line as
// abnf-oracle, so scripts/differential.py can drive both the same way.
//
// It is deliberately thin. Every decision about what to compare lives in the Python script;
// this only answers three questions about one grammar at a time:
//
//	load     <grammar>                        does go-abnf accept this grammar at all?
//	match    <grammar> <rule> <input-file>    does it accept this input?
//	matchdir <grammar> <rule> <dir>           the same for every file in a directory
//	gen      <grammar> <rule> <seed> <n> <d>  what does it generate?
//
// `matchdir` exists because parsing a grammar is the expensive part: RFC 5322 takes long
// enough that one process per input made the differential run take minutes. It loads once and
// prints one `name<TAB>verdict` line per file, mirroring `abnf-oracle match --dir`.
//
// Exit codes follow abnf-oracle's convention (SCOPE.md 11), because the point is to compare
// verdicts and a different convention on each side would invite mistakes: 0 the answer is yes,
// 1 the answer is no, 2 the question could not be answered.
package main

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	goabnf "github.com/pandatix/go-abnf"
)

const (
	exitYes   = 0
	exitNo    = 1
	exitError = 2
)

func main() {
	if len(os.Args) < 2 {
		fail("usage: goharness load|match|gen ...")
	}
	switch os.Args[1] {
	case "load":
		cmdLoad(os.Args[2:])
	case "match":
		cmdMatch(os.Args[2:])
	case "matchdir":
		cmdMatchDir(os.Args[2:])
	case "gen":
		cmdGen(os.Args[2:])
	default:
		fail("unknown command %q", os.Args[1])
	}
}

func fail(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(exitError)
}

// redefineCoreRules reports whether the caller asked for core-rule shadowing to be permitted.
//
// go-abnf refuses it by default and warns that enabling it requires "an isomorphism between the
// core rule and the redefinition" -- the redefinition applies everywhere, so it has to mean the
// same thing as what it replaced. That precondition is what the comparison is about, so it is a
// flag rather than always on.
func redefineCoreRules() bool {
	return os.Getenv("GOHARNESS_REDEFINE_CORE") == "1"
}

// loadGrammar reads a grammar file and hands it to go-abnf.
//
// The source is normalised to CRLF first. Both implementations want it and the repository
// stores fixtures with LF, so leaving it to the checkout would make results depend on git
// settings rather than on the grammars.
func loadGrammar(path string) (*goabnf.Grammar, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	text := strings.ReplaceAll(string(raw), "\r\n", "\n")
	text = strings.ReplaceAll(text, "\n", "\r\n")

	opts := []goabnf.ABNFOption{}
	if redefineCoreRules() {
		opts = append(opts, goabnf.WithRedefineCoreRules(true))
	}
	return goabnf.ParseABNF([]byte(text), opts...)
}

func cmdLoad(args []string) {
	if len(args) != 1 {
		fail("usage: goharness load <grammar>")
	}
	grammar, err := loadGrammar(args[0])
	if err != nil {
		fail("%v", err)
	}
	fmt.Printf("%d rules\n", len(grammar.Rulemap))
	os.Exit(exitYes)
}

func cmdMatch(args []string) {
	if len(args) != 3 {
		fail("usage: goharness match <grammar> <rule> <input-file>")
	}
	grammar, err := loadGrammar(args[0])
	if err != nil {
		fail("%v", err)
	}
	input, err := os.ReadFile(args[2])
	if err != nil {
		fail("%v", err)
	}

	// go-abnf panics on some inputs rather than returning an error. A panic is not a verdict,
	// so it becomes exit 2 like any other failure to decide.
	defer func() {
		if r := recover(); r != nil {
			fail("panic: %v", r)
		}
	}()

	switch decide(grammar, args[1], input) {
	case "ACCEPT":
		os.Exit(exitYes)
	case "REJECT":
		os.Exit(exitNo)
	}
	fail("could not decide")
}

// decide answers one input, converting a panic into "could not decide".
//
// go-abnf panics on some inputs rather than returning an error. A panic is not a verdict, so it
// is reported as undecided rather than as a rejection -- the same distinction this crate draws
// between "does not match" and "could not decide" (D14).
func decide(grammar *goabnf.Grammar, rule string, input []byte) (verdict string) {
	defer func() {
		if r := recover(); r != nil {
			verdict = "ERROR"
		}
	}()
	ok, err := grammar.IsValid(rule, input)
	if err != nil {
		return "ERROR"
	}
	if ok {
		return "ACCEPT"
	}
	return "REJECT"
}

func cmdMatchDir(args []string) {
	if len(args) != 3 {
		fail("usage: goharness matchdir <grammar> <rule> <dir>")
	}
	grammar, err := loadGrammar(args[0])
	if err != nil {
		fail("%v", err)
	}
	entries, err := os.ReadDir(args[2])
	if err != nil {
		fail("%v", err)
	}

	// Flushed explicitly below, not by defer: os.Exit does not run deferred functions, so
	// a deferred flush would discard every verdict, and the caller would read the empty
	// output as "could not decide" rather than as a failure.
	out := bufio.NewWriter(os.Stdout)
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		input, err := os.ReadFile(filepath.Join(args[2], entry.Name()))
		if err != nil {
			fmt.Fprintf(out, "%s\tERROR\n", entry.Name())
			continue
		}
		fmt.Fprintf(out, "%s\t%s\n", entry.Name(), decide(grammar, args[1], input))
	}
	if err := out.Flush(); err != nil {
		fail("%v", err)
	}
	os.Exit(exitYes)
}

func cmdGen(args []string) {
	if len(args) != 5 {
		fail("usage: goharness gen <grammar> <rule> <seed> <count> <out-dir>")
	}
	grammar, err := loadGrammar(args[0])
	if err != nil {
		fail("%v", err)
	}
	rule := args[1]
	seed, err := strconv.ParseInt(args[2], 10, 64)
	if err != nil {
		fail("bad seed: %v", err)
	}
	count, err := strconv.Atoi(args[3])
	if err != nil {
		fail("bad count: %v", err)
	}
	outDir := args[4]
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		fail("%v", err)
	}

	defer func() {
		if r := recover(); r != nil {
			fail("panic: %v", r)
		}
	}()

	written := 0
	for i := 0; i < count; i++ {
		// A fresh seed per string: Generate takes the seed rather than carrying state, so
		// reusing one would produce the same string every time.
		produced, err := grammar.Generate(seed+int64(i), rule)
		if err != nil {
			// Some rules it cannot expand. Not a disagreement about the language, so it is
			// reported and skipped rather than failing the run.
			fmt.Fprintf(os.Stderr, "skip %s #%d: %v\n", rule, i, err)
			continue
		}
		name := filepath.Join(outDir, fmt.Sprintf("%04d.txt", written))
		if err := os.WriteFile(name, produced, 0o644); err != nil {
			fail("%v", err)
		}
		written++
	}
	fmt.Printf("%d\n", written)
	os.Exit(exitYes)
}
