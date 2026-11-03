package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) OptionNoneParam(ctx context.Context, a *string) error {
	if a != nil {
		panic(fmt.Sprintf("expected none, got `%s`", *a))
	}
	return nil
}

func (handler) OptionSomeParam(ctx context.Context, a *string) error {
	if a == nil || *a != "foo" {
		panic(fmt.Sprintf("expected `foo`, got %v", a))
	}
	return nil
}

func (handler) OptionNoneResult(ctx context.Context) (*string, error) {
	return nil, nil
}

func (handler) OptionSomeResult(ctx context.Context) (*string, error) {
	v := "foo"
	return &v, nil
}

func (handler) OptionRoundtrip(ctx context.Context, a *string) (*string, error) {
	return a, nil
}

func (handler) DoubleOptionRoundtrip(ctx context.Context, a **uint32) (**uint32, error) {
	return a, nil
}
