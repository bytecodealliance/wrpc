package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) OneArgument(ctx context.Context, x uint32) error {
	assertEqual(x, 1)
	return nil
}

func (handler) OneResult(ctx context.Context) (uint32, error) {
	return 2, nil
}

func (handler) OneArgumentAndResult(ctx context.Context, x uint32) (uint32, error) {
	assertEqual(x, 3)
	return 4, nil
}

func (handler) TwoArguments(ctx context.Context, x uint32, y uint32) error {
	assertEqual(x, 5)
	assertEqual(y, 6)
	return nil
}

func (handler) TwoArgumentsAndResult(ctx context.Context, x uint32, y uint32) (uint32, error) {
	assertEqual(x, 7)
	assertEqual(y, 8)
	return 9, nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
