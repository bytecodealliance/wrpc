package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/my/inline/foo"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Bar(ctx context.Context) (wrpc.Own[foo.Bar], error) {
	return wrpc.Own[foo.Bar]{}, nil
}
