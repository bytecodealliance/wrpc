package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_with_lists/test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	thingInstance := must(test.NewThing(ctx, c, []byte("Hi")))
	assertEqual(string(must(test.Thing_Foo(ctx, c, thingInstance.Borrow()))), "Hi Thing HostThing HostThing.foo Thing.foo")
	if err := test.Thing_Bar(ctx, c, thingInstance.Borrow(), []byte("Hola")); err != nil {
		return err
	}
	assertEqual(string(must(test.Thing_Foo(ctx, c, thingInstance.Borrow()))), "Hola Thing.bar HostThing.bar HostThing.foo Thing.foo")
	assertEqual(string(must(test.Thing_Baz(ctx, c, []byte("Ohayo Gozaimas")))), "Ohayo Gozaimas Thing.baz HostThing.baz Thing.baz again")
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
