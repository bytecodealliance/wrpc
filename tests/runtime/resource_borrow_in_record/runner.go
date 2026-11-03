package runner

import (
	"context"
	"fmt"
	"slices"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_borrow_in_record/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	thing1 := must(to_test.NewThing(ctx, c, "Bonjour"))
	thing2 := must(to_test.NewThing(ctx, c, "mon cher"))
	things := must(to_test.Test(ctx, c, []*to_test.Foo{
		{Thing: thing1.Borrow()},
		{Thing: thing2.Borrow()},
	}))
	var result []string
	for _, thing := range things {
		result = append(result, must(to_test.Thing_Get(ctx, c, thing.Borrow())))
	}
	if expected := []string{"Bonjour new test get", "mon cher new test get"}; !slices.Equal(result, expected) {
		panic(fmt.Sprintf("%v not equal to %v", result, expected))
	}
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}
