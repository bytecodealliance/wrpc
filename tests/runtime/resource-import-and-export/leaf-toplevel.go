package leaf_toplevel

import (
	"context"

	wrpc "wrpc.io/go"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ToplevelExport(ctx context.Context, a wrpc.Own[Thing]) (wrpc.Own[Thing], error) {
	return a, nil
}
