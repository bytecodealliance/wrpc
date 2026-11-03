//@ args = '--features y'

package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Y(ctx context.Context) error {
	return nil
}

func (handler) Z(ctx context.Context) error {
	return nil
}
