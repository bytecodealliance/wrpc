package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/list_in_variant/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	assertEqual(must(to_test.ListInOption(ctx, c, []string{"hello", "world"})), "hello,world")
	assertEqual(must(to_test.ListInOption(ctx, c, nil)), "none")

	fbb := to_test.NewPayloadOrEmptyWithData([]string{"foo", "bar", "baz"})
	assertEqual(must(to_test.ListInVariant(ctx, c, fbb)), "foo,bar,baz")
	assertEqual(must(to_test.ListInVariant(ctx, c, to_test.NewPayloadOrEmptyEmpty())), "empty")

	abc := wrpc.Ok[string]([]string{"a", "b", "c"})
	assertEqual(must(to_test.ListInResult(ctx, c, abc)), "a,b,c")
	oops := wrpc.Err[[]string]("oops")
	assertEqual(must(to_test.ListInResult(ctx, c, oops)), "err:oops")

	s := must(to_test.ListInOptionWithReturn(ctx, c, []string{"hello", "world"}))
	assertEqual(s.Count, 2)
	assertEqual(s.Label, "hello,world")
	s = must(to_test.ListInOptionWithReturn(ctx, c, nil))
	assertEqual(s.Count, 0)
	assertEqual(s.Label, "none")

	assertEqual(must(to_test.TopLevelList(ctx, c, []string{"x", "y", "z"})), "x,y,z")
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
