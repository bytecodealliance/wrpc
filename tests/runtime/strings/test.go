package test

import (
	"context"
	"fmt"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) TakeBasic(ctx context.Context, x string) error {
	if x != "latin utf16" {
		panic(fmt.Sprintf("unexpected value: `%v`", x))
	}
	return nil
}

func (handler) ReturnUnicode(ctx context.Context) (string, error) {
	return "🚀🚀🚀 𠈄𓀀", nil
}

func (handler) ReturnEmpty(ctx context.Context) (string, error) {
	return "", nil
}

func (handler) Roundtrip(ctx context.Context, x string) (string, error) {
	return x, nil
}
