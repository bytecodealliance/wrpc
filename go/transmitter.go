package wrpc

import (
	"context"
)

type Transmitter interface {
	Transmit(context.Context, []byte, ...uint32) error
}

type NestedTransmitter struct {
	Transmitter
	Path []uint32
}

func (tx *NestedTransmitter) Transmit(ctx context.Context, b []byte, path ...uint32) error {
	return tx.Transmitter.Transmit(ctx, b, append(tx.Path, path...)...)
}

func NewNestedTransmitter(tx Transmitter, path ...uint32) *NestedTransmitter {
	return &NestedTransmitter{tx, path}
}
