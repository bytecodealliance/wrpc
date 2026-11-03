//@ args = '--generate-unused-types'

package test

import (
	"context"

	"driver/test/exports/foo/bar/component"
)

var (
	_ component.UnusedEnum
	_ component.UnusedRecord
	_ component.UnusedVariant
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Foo(ctx context.Context) error {
	return nil
}
