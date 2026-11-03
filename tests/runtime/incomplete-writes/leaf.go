package leaf

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/leaf/exports/my/test/leaf_interface"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) LeafThing(ctx context.Context, s string) (wrpc.Own[leaf_interface.LeafThing], error) {
	return wrpc.Own[leaf_interface.LeafThing](s), nil
}

func (handler) LeafThing_Get(ctx context.Context, self wrpc.Borrow[leaf_interface.LeafThing]) (string, error) {
	return string(self), nil
}
