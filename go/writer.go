package wrpc

import (
	"context"
)

type TransmitWriter struct {
	ctx  context.Context
	tx   Transmitter
	path []uint32
}

func NewTransmitWriter(ctx context.Context, tx Transmitter, path ...uint32) *TransmitWriter {
	return &TransmitWriter{
		ctx, tx, path,
	}
}

func (w *TransmitWriter) Write(b []byte) (int, error) {
	if err := w.tx.Transmit(w.ctx, b, w.path...); err != nil {
		return 0, err
	}
	return len(b), nil
}
