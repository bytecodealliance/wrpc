package wrpctcp

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"net"

	wrpc "wrpc.io/go"
)

// Client is a thin wrapper around *net.TCPAddr, which is able to invoke wRPC functions
type Client struct {
	addr string
}

func NewClient(addr string) *Client {
	return &Client{addr: addr}
}

type paramWriter struct {
	conn *net.TCPConn
}

func (w *paramWriter) Write([]byte) (int, error) {
	return 0, errors.New("not supported yet")
}

func (w *paramWriter) WriteByte(byte) error {
	return errors.New("not supported yet")
}

func (*paramWriter) Index(path ...uint32) (wrpc.IndexWriteCloser, error) {
	return nil, errors.New("not supported yet")
}

func (w *paramWriter) Close() error {
	return nil
	// TODO: close the write side of connection
	// return w.conn.CloseWrite()
}

type streamReader struct {
	conn   *net.TCPConn
	buffer *bytes.Buffer
}

func (r *streamReader) Read(p []byte) (int, error) {
	return r.buffer.Read(p)
}

func (r *streamReader) ReadByte() (byte, error) {
	return r.buffer.ReadByte()
}

func (r *streamReader) Index(path ...uint32) (wrpc.IndexReadCloser, error) {
	return nil, errors.New("not supported yet")
}

func (r *streamReader) Close() error {
	return nil
	// TODO: close the write side of connection
	// return r.conn.CloseRead()
}

func (c *Client) Invoke(ctx context.Context, instance string, name string, buf []byte, paths ...wrpc.SubscribePath) (wrpc.IndexWriteCloser, wrpc.IndexReadCloser, error) {
	addr, err := net.ResolveTCPAddr("tcp", c.addr)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to resolve `%s` as TCP address: %w", c.addr, err)
	}
	conn, err := net.DialTCP("tcp", nil, addr)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to establish TCP connection: %w", err)
	}

	if len(instance) > math.MaxUint32 {
		return nil, nil, errors.New("instance length overflows uint32")
	}
	if len(name) > math.MaxUint32 {
		return nil, nil, errors.New("name length overflows uint32")
	}
	if len(buf) > math.MaxUint32 {
		return nil, nil, errors.New("buffer length overflows uint32")
	}
	// TODO: Count the exact amount of bytes required
	payload := make(
		[]byte,
		1+ // version
			binary.MaxVarintLen32+len(instance)+
			binary.MaxVarintLen32+len(name)+
			1+ // empty path length
			binary.MaxVarintLen32+len(buf),
	)
	r := 1 // version == 0

	l := r + binary.PutUvarint(payload[r:], uint64(len(instance)))
	r = l + len(instance)
	copy(payload[l:r], instance)

	l = r + binary.PutUvarint(payload[r:], uint64(len(name)))
	r = l + len(name)
	copy(payload[l:r], name)

	l = r + 1 /* empty path length */ + binary.PutUvarint(payload[r:], uint64(len(buf)))
	r = l + len(buf)
	copy(payload[l:r], buf)

	_, err = conn.Write(payload[:r])
	if err != nil {
		return nil, nil, fmt.Errorf("failed to write initial frame: %w", err)
	}
	if err = conn.CloseWrite(); err != nil {
		return nil, nil, fmt.Errorf("failed to close write side of TCP connection: %w", err)
	}
	var header [2]byte
	_, err = io.ReadFull(conn, header[:])
	if err != nil {
		return nil, nil, fmt.Errorf("failed to read response frame header: %w", err)
	}
	if header[0] != 0 {
		return nil, nil, errors.New("asynchronous frames not supported yet")
	}
	n := header[1]
	if n > 127 {
		return nil, nil, errors.New("only decoding responses of up to 127 bytes is currently supported")
	}
	buf = make([]byte, n)
	_, err = io.ReadFull(conn, buf)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to read response frame payload: %w", err)
	}
	if err := conn.Close(); err != nil {
		return nil, nil, fmt.Errorf("failed to close TCP connection: %w", err)
	}
	return &paramWriter{
			conn: conn,
		}, &streamReader{
			conn:   conn,
			buffer: bytes.NewBuffer(buf),
		}, nil
}
