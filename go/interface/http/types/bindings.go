package types

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"log/slog"
	"math"

	wrpc "github.com/wrpc/wrpc/go"
	"golang.org/x/sync/errgroup"
)

type MethodDiscriminant uint32

const (
	MethodDiscriminant_Get MethodDiscriminant = iota
	MethodDiscriminant_Head
	MethodDiscriminant_Post
	MethodDiscriminant_Put
	MethodDiscriminant_Delete
	MethodDiscriminant_Connect
	MethodDiscriminant_Options
	MethodDiscriminant_Trace
	MethodDiscriminant_Patch
	MethodDiscriminant_Other
)

type MethodVariant struct {
	payload      any
	discriminant MethodDiscriminant
}

func NewMethod_Get() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Get}
}

func NewMethod_Head() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Head}
}

func NewMethod_Post() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Post}
}

func NewMethod_Delete() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Delete}
}

func NewMethod_Connect() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Connect}
}

func NewMethod_Options() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Options}
}

func NewMethod_Trace() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Trace}
}

func NewMethod_Patch() *MethodVariant {
	return &MethodVariant{nil, MethodDiscriminant_Patch}
}

func NewMethod_Other(payload string) *MethodVariant {
	return &MethodVariant{payload, MethodDiscriminant_Other}
}

func (v *MethodVariant) Discriminant() MethodDiscriminant {
	return v.discriminant
}

func (v *MethodVariant) GetOther() (string, bool) {
	if v.discriminant != MethodDiscriminant_Other {
		return "", false
	}
	p, ok := v.payload.(string)
	return p, ok
}

func (v *MethodVariant) String() string {
	switch v.discriminant {
	case MethodDiscriminant_Get:
		return "get"
	case MethodDiscriminant_Head:
		return "head"
	case MethodDiscriminant_Post:
		return "post"
	case MethodDiscriminant_Delete:
		return "delete"
	case MethodDiscriminant_Connect:
		return "connect"
	case MethodDiscriminant_Options:
		return "options"
	case MethodDiscriminant_Trace:
		return "trace"
	case MethodDiscriminant_Patch:
		return "patch"
	case MethodDiscriminant_Other:
		return "other"
	default:
		panic("unreachable")
	}
}

func ReadMethod(r wrpc.ByteReader) (*MethodVariant, error) {
	disc, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `method` discriminant: %w", err)
	}
	switch MethodDiscriminant(disc) {
	case MethodDiscriminant_Get:
		return NewMethod_Get(), nil
	case MethodDiscriminant_Head:
		return NewMethod_Head(), nil
	case MethodDiscriminant_Post:
		return NewMethod_Post(), nil
	case MethodDiscriminant_Delete:
		return NewMethod_Delete(), nil
	case MethodDiscriminant_Connect:
		return NewMethod_Connect(), nil
	case MethodDiscriminant_Options:
		return NewMethod_Options(), nil
	case MethodDiscriminant_Trace:
		return NewMethod_Trace(), nil
	case MethodDiscriminant_Patch:
		return NewMethod_Patch(), nil
	case MethodDiscriminant_Other:
		payload, err := wrpc.ReadString(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `method::other` value: %w", err)
		}
		return NewMethod_Other(payload), nil
	default:
		return nil, fmt.Errorf("unknown `method` discriminant value %d", disc)
	}
}

type SchemeDiscriminant uint32

const (
	SchemeDiscriminant_Http SchemeDiscriminant = iota
	SchemeDiscriminant_Https
	SchemeDiscriminant_Other
)

type SchemeVariant struct {
	payload      any
	discriminant SchemeDiscriminant
}

func NewScheme_Http() *SchemeVariant {
	return &SchemeVariant{nil, SchemeDiscriminant_Http}
}

func NewScheme_Https() *SchemeVariant {
	return &SchemeVariant{nil, SchemeDiscriminant_Https}
}

func NewScheme_Other(payload string) *SchemeVariant {
	return &SchemeVariant{payload, SchemeDiscriminant_Other}
}

func (v *SchemeVariant) Discriminant() SchemeDiscriminant {
	return v.discriminant
}

func (v *SchemeVariant) GetOther() (string, bool) {
	if v.discriminant != SchemeDiscriminant_Other {
		return "", false
	}
	p, ok := v.payload.(string)
	return p, ok
}

func (v *SchemeVariant) String() string {
	switch v.discriminant {
	case SchemeDiscriminant_Http:
		return "http"
	case SchemeDiscriminant_Https:
		return "https"
	case SchemeDiscriminant_Other:
		return "other"
	default:
		panic("unreachable")
	}
}

func ReadScheme(r wrpc.ByteReader) (*SchemeVariant, error) {
	disc, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `scheme` discriminant: %w", err)
	}
	switch SchemeDiscriminant(disc) {
	case SchemeDiscriminant_Http:
		return NewScheme_Http(), nil
	case SchemeDiscriminant_Https:
		return NewScheme_Https(), nil
	case SchemeDiscriminant_Other:
		payload, err := wrpc.ReadString(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `scheme::other` value: %w", err)
		}
		return NewScheme_Other(payload), nil
	default:
		return nil, fmt.Errorf("unknown `scheme` discriminant value %d", disc)
	}
}

type ErrorCodeDiscriminant uint32

const (
	ErrorCodeDiscriminant_DNSTimeout ErrorCodeDiscriminant = iota
	ErrorCodeDiscriminant_DNSError
	ErrorCodeDiscriminant_DestinationNotFound
	ErrorCodeDiscriminant_DestinationUnavailable
	ErrorCodeDiscriminant_DestinationIPProhibited
	ErrorCodeDiscriminant_DestinationIPUnroutable
	ErrorCodeDiscriminant_ConnectionRefused
	ErrorCodeDiscriminant_ConnectionTerminated
	ErrorCodeDiscriminant_ConnectionTimeout
	ErrorCodeDiscriminant_ConnectionReadTimeout
	ErrorCodeDiscriminant_ConnectionWriteTimeout
	ErrorCodeDiscriminant_ConnectionLimitReached
	ErrorCodeDiscriminant_TLSProtocolError
	ErrorCodeDiscriminant_TLSCertificateError
	ErrorCodeDiscriminant_TLSAlertReceived
	ErrorCodeDiscriminant_HTTPRequestDenied
	ErrorCodeDiscriminant_HTTPRequestLengthRequired
	ErrorCodeDiscriminant_HTTPRequestBodySize
	ErrorCodeDiscriminant_HTTPRequestMethodInvalid
	ErrorCodeDiscriminant_HTTPRequestUriInvalid
	ErrorCodeDiscriminant_HTTPRequestUriTooLong
	ErrorCodeDiscriminant_HTTPRequestHeaderSectionSize
	ErrorCodeDiscriminant_HTTPRequestHeaderSize
	ErrorCodeDiscriminant_HTTPRequestTrailerSectionSize
	ErrorCodeDiscriminant_HTTPRequestTrailerSize
	ErrorCodeDiscriminant_HTTPResponseIncomplete
	ErrorCodeDiscriminant_HTTPResponseHeaderSectionSize
	ErrorCodeDiscriminant_HTTPResponseHeaderSize
	ErrorCodeDiscriminant_HTTPResponseBodySize
	ErrorCodeDiscriminant_HTTPResponseTrailerSectionSize
	ErrorCodeDiscriminant_HTTPResponseTrailerSize
	ErrorCodeDiscriminant_HTTPResponseTransferCoding
	ErrorCodeDiscriminant_HTTPResponseContentCoding
	ErrorCodeDiscriminant_HTTPResponseTimeout
	ErrorCodeDiscriminant_HTTPUpgradeFailed
	ErrorCodeDiscriminant_HTTPProtocolError
	ErrorCodeDiscriminant_LoopDetected
	ErrorCodeDiscriminant_ConfigurationError
	ErrorCodeDiscriminant_InternalError
)

func NewErrorCode_DNSTimeout() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_DNSTimeout}
}

func NewErrorCode_DNSError(payload *DNSErrorPayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_DNSError}
}

func NewErrorCode_DestinationNotFound() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_DestinationNotFound}
}

func NewErrorCode_DestinationUnavailable() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_DestinationUnavailable}
}

func NewErrorCode_DestinationIPProhibited() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_DestinationIPProhibited}
}

func NewErrorCode_DestinationIPUnroutable() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_DestinationIPUnroutable}
}

func NewErrorCode_ConnectionRefused() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionRefused}
}

func NewErrorCode_ConnectionTerminated() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionTerminated}
}

func NewErrorCode_ConnectionTimeout() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionTimeout}
}

func NewErrorCode_ConnectionReadTimeout() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionReadTimeout}
}

func NewErrorCode_ConnectionWriteTimeout() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionWriteTimeout}
}

func NewErrorCode_ConnectionLimitReached() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConnectionLimitReached}
}

func NewErrorCode_TLSProtocolError() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_TLSProtocolError}
}

func NewErrorCode_TLSCertificateError() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_TLSCertificateError}
}

func NewErrorCode_TLSAlertReceived(payload *TLSAlertReceivedPayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_TLSAlertReceived}
}

func NewErrorCode_HTTPRequestDenied() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPRequestDenied}
}

func NewErrorCode_HTTPRequestLengthRequired() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPRequestLengthRequired}
}

func NewErrorCode_HTTPRequestBodySize(payload *uint64) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPRequestBodySize}
}

func NewErrorCode_HTTPRequestMethodInvalid() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPRequestMethodInvalid}
}

func NewErrorCode_HTTPRequestUriInvalid() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPRequestUriInvalid}
}

func NewErrorCode_HTTPRequestUriTooLong() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPRequestUriTooLong}
}

func NewErrorCode_HTTPRequestHeaderSectionSize(payload *uint32) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPRequestHeaderSectionSize}
}

func NewErrorCode_HTTPRequestHeaderSize(payload *FieldSizePayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPRequestHeaderSize}
}

func NewErrorCode_HTTPRequestTrailerSectionSize(payload *uint32) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPRequestTrailerSectionSize}
}

func NewErrorCode_HTTPRequestTrailerSize(payload *FieldSizePayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPRequestTrailerSize}
}

func NewErrorCode_HTTPResponseIncomplete() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPResponseIncomplete}
}

func NewErrorCode_HTTPResponseHeaderSectionSize(payload *uint32) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseHeaderSectionSize}
}

func NewErrorCode_HTTPResponseHeaderSize(payload *FieldSizePayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseHeaderSize}
}

func NewErrorCode_HTTPResponseBodySize(payload *uint64) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseBodySize}
}

func NewErrorCode_HTTPResponseTrailerSectionSize(payload *uint32) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseTrailerSectionSize}
}

func NewErrorCode_HTTPResponseTrailerSize(payload *FieldSizePayloadRecord) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseTrailerSize}
}

func NewErrorCode_HTTPResponseTransferCoding(payload *string) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseTransferCoding}
}

func NewErrorCode_HTTPResponseContentCoding(payload *string) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_HTTPResponseContentCoding}
}

func NewErrorCode_HTTPResponseTimeout() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPResponseTimeout}
}

func NewErrorCode_HTTPUpgradeFailed() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPUpgradeFailed}
}

func NewErrorCode_HTTPProtocolError() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_HTTPProtocolError}
}

func NewErrorCode_LoopDetected() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_LoopDetected}
}

func NewErrorCode_ConfigurationError() *ErrorCodeVariant {
	return &ErrorCodeVariant{nil, ErrorCodeDiscriminant_ConfigurationError}
}

func NewErrorCode_InternalError(payload *string) *ErrorCodeVariant {
	return &ErrorCodeVariant{payload, ErrorCodeDiscriminant_InternalError}
}

type ErrorCodeVariant struct {
	payload      any
	discriminant ErrorCodeDiscriminant
}

func (v *ErrorCodeVariant) Discriminant() ErrorCodeDiscriminant {
	return v.discriminant
}

func (v *ErrorCodeVariant) String() string {
	switch v.discriminant {
	case ErrorCodeDiscriminant_DNSTimeout:
		return "DNS-timeout"
	case ErrorCodeDiscriminant_DNSError:
		return "DNS-error"
	case ErrorCodeDiscriminant_DestinationNotFound:
		return "destination-not-found"
	case ErrorCodeDiscriminant_DestinationUnavailable:
		return "destination-unavailable"
	case ErrorCodeDiscriminant_DestinationIPProhibited:
		return "destination-IP-prohibited"
	case ErrorCodeDiscriminant_DestinationIPUnroutable:
		return "destination-IP-unroutable"
	case ErrorCodeDiscriminant_ConnectionRefused:
		return "connection-refused"
	case ErrorCodeDiscriminant_ConnectionTerminated:
		return "connection-terminated"
	case ErrorCodeDiscriminant_ConnectionTimeout:
		return "connection-timeout"
	case ErrorCodeDiscriminant_ConnectionReadTimeout:
		return "connection-read-timeout"
	case ErrorCodeDiscriminant_ConnectionWriteTimeout:
		return "connection-write-timeout"
	case ErrorCodeDiscriminant_ConnectionLimitReached:
		return "connection-limit-reached"
	case ErrorCodeDiscriminant_TLSProtocolError:
		return "TLS-protocol-error"
	case ErrorCodeDiscriminant_TLSCertificateError:
		return "TLS-certificate-error"
	case ErrorCodeDiscriminant_TLSAlertReceived:
		return "TLS-alert-received"
	case ErrorCodeDiscriminant_HTTPRequestDenied:
		return "HTTP-request-denied"
	case ErrorCodeDiscriminant_HTTPRequestLengthRequired:
		return "HTTP-request-length-required"
	case ErrorCodeDiscriminant_HTTPRequestBodySize:
		return "HTTP-request-body-size"
	case ErrorCodeDiscriminant_HTTPRequestMethodInvalid:
		return "HTTP-request-method-invalid"
	case ErrorCodeDiscriminant_HTTPRequestUriInvalid:
		return "HTTP-request-uri-invalid"
	case ErrorCodeDiscriminant_HTTPRequestUriTooLong:
		return "HTTP-request-uri-too-long"
	case ErrorCodeDiscriminant_HTTPRequestHeaderSectionSize:
		return "HTTP-request-header-section-size"
	case ErrorCodeDiscriminant_HTTPRequestHeaderSize:
		return "HTTP-request-header-size"
	case ErrorCodeDiscriminant_HTTPRequestTrailerSectionSize:
		return "HTTP-request-trailer-section-size"
	case ErrorCodeDiscriminant_HTTPRequestTrailerSize:
		return "HTTP-request-trailer-size"
	case ErrorCodeDiscriminant_HTTPResponseIncomplete:
		return "HTTP-response-incomplete"
	case ErrorCodeDiscriminant_HTTPResponseHeaderSectionSize:
		return "HTTP-response-header-section-size"
	case ErrorCodeDiscriminant_HTTPResponseHeaderSize:
		return "HTTP-response-header-size"
	case ErrorCodeDiscriminant_HTTPResponseBodySize:
		return "HTTP-response-body-size"
	case ErrorCodeDiscriminant_HTTPResponseTrailerSectionSize:
		return "HTTP-response-trailer-section-size"
	case ErrorCodeDiscriminant_HTTPResponseTrailerSize:
		return "HTTP-response-trailer-size"
	case ErrorCodeDiscriminant_HTTPResponseTransferCoding:
		return "HTTP-response-transfer-coding"
	case ErrorCodeDiscriminant_HTTPResponseContentCoding:
		return "HTTP-response-content-coding"
	case ErrorCodeDiscriminant_HTTPResponseTimeout:
		return "HTTP-response-timeout"
	case ErrorCodeDiscriminant_HTTPUpgradeFailed:
		return "HTTP-upgrade-failed"
	case ErrorCodeDiscriminant_HTTPProtocolError:
		return "http-protocol-error"
	case ErrorCodeDiscriminant_LoopDetected:
		return "loop-detected"
	case ErrorCodeDiscriminant_ConfigurationError:
		return "configuration-error"
	case ErrorCodeDiscriminant_InternalError:
		return "internal-error"
	default:
		panic("unreachable")
	}
}

func ReadErrorCode(r wrpc.ByteReader) (*ErrorCodeVariant, error) {
	disc, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `error-code` discriminant: %w", err)
	}
	switch ErrorCodeDiscriminant(disc) {
	case ErrorCodeDiscriminant_DNSTimeout:
		return NewErrorCode_DNSTimeout(), nil
	case ErrorCodeDiscriminant_DNSError:
		payload, err := ReadDNSErrorPayload(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_DNSError(payload), nil
	case ErrorCodeDiscriminant_DestinationNotFound:
		return NewErrorCode_DestinationNotFound(), nil
	case ErrorCodeDiscriminant_DestinationUnavailable:
		return NewErrorCode_DestinationUnavailable(), nil
	case ErrorCodeDiscriminant_DestinationIPProhibited:
		return NewErrorCode_DestinationIPProhibited(), nil
	case ErrorCodeDiscriminant_DestinationIPUnroutable:
		return NewErrorCode_DestinationIPUnroutable(), nil
	case ErrorCodeDiscriminant_ConnectionRefused:
		return NewErrorCode_ConnectionRefused(), nil
	case ErrorCodeDiscriminant_ConnectionTerminated:
		return NewErrorCode_ConnectionTerminated(), nil
	case ErrorCodeDiscriminant_ConnectionTimeout:
		return NewErrorCode_ConnectionTimeout(), nil
	case ErrorCodeDiscriminant_ConnectionReadTimeout:
		return NewErrorCode_ConnectionReadTimeout(), nil
	case ErrorCodeDiscriminant_ConnectionWriteTimeout:
		return NewErrorCode_ConnectionWriteTimeout(), nil
	case ErrorCodeDiscriminant_ConnectionLimitReached:
		return NewErrorCode_ConnectionLimitReached(), nil
	case ErrorCodeDiscriminant_TLSProtocolError:
		return NewErrorCode_TLSProtocolError(), nil
	case ErrorCodeDiscriminant_TLSCertificateError:
		return NewErrorCode_TLSCertificateError(), nil
	case ErrorCodeDiscriminant_TLSAlertReceived:
		payload, err := ReadTLSAlertReceivedPayload(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_TLSAlertReceived(payload), nil
	case ErrorCodeDiscriminant_HTTPRequestDenied:
		return NewErrorCode_HTTPRequestDenied(), nil
	case ErrorCodeDiscriminant_HTTPRequestLengthRequired:
		return NewErrorCode_HTTPRequestLengthRequired(), nil
	case ErrorCodeDiscriminant_HTTPRequestBodySize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint64)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPRequestBodySize(payload), nil
	case ErrorCodeDiscriminant_HTTPRequestMethodInvalid:
		return NewErrorCode_HTTPRequestMethodInvalid(), nil
	case ErrorCodeDiscriminant_HTTPRequestUriInvalid:
		return NewErrorCode_HTTPRequestUriInvalid(), nil
	case ErrorCodeDiscriminant_HTTPRequestUriTooLong:
		return NewErrorCode_HTTPRequestUriTooLong(), nil
	case ErrorCodeDiscriminant_HTTPRequestHeaderSectionSize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint32)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPRequestHeaderSectionSize(payload), nil
	case ErrorCodeDiscriminant_HTTPRequestHeaderSize:
		payload, err := wrpc.ReadFlatOption(r, ReadFieldSizePayload)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPRequestHeaderSize(payload), nil
	case ErrorCodeDiscriminant_HTTPRequestTrailerSectionSize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint32)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPRequestTrailerSectionSize(payload), nil
	case ErrorCodeDiscriminant_HTTPRequestTrailerSize:
		payload, err := ReadFieldSizePayload(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPRequestTrailerSize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseIncomplete:
		return NewErrorCode_HTTPResponseIncomplete(), nil
	case ErrorCodeDiscriminant_HTTPResponseHeaderSectionSize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint32)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseHeaderSectionSize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseHeaderSize:
		payload, err := ReadFieldSizePayload(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseHeaderSize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseBodySize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint64)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseBodySize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseTrailerSectionSize:
		payload, err := wrpc.ReadOption(r, wrpc.ReadUint32)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseTrailerSectionSize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseTrailerSize:
		payload, err := ReadFieldSizePayload(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseTrailerSize(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseTransferCoding:
		payload, err := wrpc.ReadOption(r, wrpc.ReadString)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseTransferCoding(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseContentCoding:
		payload, err := wrpc.ReadOption(r, wrpc.ReadString)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_HTTPResponseContentCoding(payload), nil
	case ErrorCodeDiscriminant_HTTPResponseTimeout:
		return NewErrorCode_HTTPResponseTimeout(), nil
	case ErrorCodeDiscriminant_HTTPUpgradeFailed:
		return NewErrorCode_HTTPUpgradeFailed(), nil
	case ErrorCodeDiscriminant_HTTPProtocolError:
		return NewErrorCode_HTTPProtocolError(), nil
	case ErrorCodeDiscriminant_LoopDetected:
		return NewErrorCode_LoopDetected(), nil
	case ErrorCodeDiscriminant_ConfigurationError:
		return NewErrorCode_ConfigurationError(), nil
	case ErrorCodeDiscriminant_InternalError:
		payload, err := wrpc.ReadOption(r, wrpc.ReadString)
		if err != nil {
			return nil, fmt.Errorf("failed to read payload: %w", err)
		}
		return NewErrorCode_InternalError(payload), nil
	default:
		return nil, fmt.Errorf("unknown `error-code` discriminant value %d", disc)
	}
}

func (v *ErrorCodeVariant) WriteToIndex(w wrpc.ByteWriter) (func(wrpc.Index[wrpc.IndexWriter]) error, error) {
	if err := wrpc.WriteUint32(uint32(v.discriminant), w); err != nil {
		return nil, fmt.Errorf("failed to write discriminant: %w", err)
	}
	switch v.discriminant {
	case ErrorCodeDiscriminant_DNSTimeout:
	case ErrorCodeDiscriminant_DNSError:
		if err := v.payload.(*DNSErrorPayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_DestinationNotFound:
	case ErrorCodeDiscriminant_DestinationUnavailable:
	case ErrorCodeDiscriminant_DestinationIPProhibited:
	case ErrorCodeDiscriminant_DestinationIPUnroutable:
	case ErrorCodeDiscriminant_ConnectionRefused:
	case ErrorCodeDiscriminant_ConnectionTerminated:
	case ErrorCodeDiscriminant_ConnectionTimeout:
	case ErrorCodeDiscriminant_ConnectionReadTimeout:
	case ErrorCodeDiscriminant_ConnectionWriteTimeout:
	case ErrorCodeDiscriminant_ConnectionLimitReached:
	case ErrorCodeDiscriminant_TLSProtocolError:
	case ErrorCodeDiscriminant_TLSCertificateError:
	case ErrorCodeDiscriminant_TLSAlertReceived:
		if err := v.payload.(*TLSAlertReceivedPayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPRequestDenied:
	case ErrorCodeDiscriminant_HTTPRequestLengthRequired:
	case ErrorCodeDiscriminant_HTTPRequestBodySize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint64(*v.payload.(*uint64), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPRequestMethodInvalid:
	case ErrorCodeDiscriminant_HTTPRequestUriInvalid:
	case ErrorCodeDiscriminant_HTTPRequestUriTooLong:
	case ErrorCodeDiscriminant_HTTPRequestHeaderSectionSize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint32(*v.payload.(*uint32), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPRequestHeaderSize:
		if v.payload == nil {
			break
		}
		if err := v.payload.(*FieldSizePayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPRequestTrailerSectionSize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint32(*v.payload.(*uint32), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPRequestTrailerSize:
		if err := v.payload.(*FieldSizePayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseIncomplete:
	case ErrorCodeDiscriminant_HTTPResponseHeaderSectionSize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint32(*v.payload.(*uint32), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseHeaderSize:
		if err := v.payload.(*FieldSizePayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseBodySize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint64(*v.payload.(*uint64), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseTrailerSectionSize:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteUint32(*v.payload.(*uint32), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseTrailerSize:
		if err := v.payload.(*FieldSizePayloadRecord).WriteTo(w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseTransferCoding:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteString(*v.payload.(*string), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseContentCoding:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteString(*v.payload.(*string), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	case ErrorCodeDiscriminant_HTTPResponseTimeout:
	case ErrorCodeDiscriminant_HTTPUpgradeFailed:
	case ErrorCodeDiscriminant_HTTPProtocolError:
	case ErrorCodeDiscriminant_LoopDetected:
	case ErrorCodeDiscriminant_ConfigurationError:
	case ErrorCodeDiscriminant_InternalError:
		if v.payload == nil {
			break
		}
		if err := wrpc.WriteString(*v.payload.(*string), w); err != nil {
			return nil, fmt.Errorf("failed to write payload: %w", err)
		}
	default:
		panic("unreachable")
	}
	return nil, nil
}

type (
	RequestSubscription struct {
		payloadBody     <-chan []byte
		payloadTrailers <-chan []byte

		stopBody     func() error
		stopTrailers func() error
	}
	RequestRecord struct {
		Body          wrpc.ReadyReader
		Trailers      wrpc.ReadyReceiver[[]*wrpc.Tuple2[string, [][]byte]]
		Method        *MethodVariant
		PathWithQuery *string
		Scheme        *SchemeVariant
		Authority     *string
		Headers       []*wrpc.Tuple2[string, [][]byte]
	}

	RequestOptionsRecord struct {
		ConnectTimeout     *uint64
		FirstByteTimeout   *uint64
		BetweenByteTimeout *uint64
	}

	ResponseSubscription struct {
		payloadBody     <-chan []byte
		payloadTrailers <-chan []byte

		stopBody     func() error
		stopTrailers func() error
	}

	ResponseRecord struct {
		Body     wrpc.ReadyReader
		Trailers wrpc.ReadyReceiver[[]*wrpc.Tuple2[string, [][]byte]]
		Status   uint16
		Headers  []*wrpc.Tuple2[string, [][]byte]
	}
)

type DNSErrorPayloadRecord struct {
	Rcode    *string
	InfoCode *uint16
}

func ReadDNSErrorPayload(r wrpc.ByteReader) (*DNSErrorPayloadRecord, error) {
	slog.Debug("reading `rcode`")
	rcode, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `rcode`: %w", err)
	}
	slog.Debug("read `rcode`", "rcode", rcode)
	slog.Debug("reading `info-code`")
	infoCode, err := wrpc.ReadOption(r, wrpc.ReadUint16)
	if err != nil {
		return nil, fmt.Errorf("failed to read `info-code`: %w", err)
	}
	slog.Debug("read `info-code`", "info-code", infoCode)
	return &DNSErrorPayloadRecord{rcode, infoCode}, nil
}

func (v *DNSErrorPayloadRecord) WriteTo(w wrpc.ByteWriter) error {
	slog.Debug("writing `rcode`")
	if err := wrpc.WriteOption(v.Rcode, w, wrpc.WriteString); err != nil {
		return fmt.Errorf("failed to write `rcode`: %w", err)
	}
	slog.Debug("writing `info-code`")
	if err := wrpc.WriteOption(v.InfoCode, w, wrpc.WriteUint16); err != nil {
		return fmt.Errorf("failed to write `info-code`: %w", err)
	}
	return nil
}

type TLSAlertReceivedPayloadRecord struct {
	AlertId      *uint8
	AlertMessage *string
}

func ReadTLSAlertReceivedPayload(r wrpc.ByteReader) (*TLSAlertReceivedPayloadRecord, error) {
	slog.Debug("reading `alert-id`")
	alertId, err := wrpc.ReadOption(r, wrpc.ByteReader.ReadByte)
	if err != nil {
		return nil, fmt.Errorf("failed to read `alert-id`: %w", err)
	}
	slog.Debug("read `alert-id`", "alert-id", alertId)
	slog.Debug("reading `alert-message`")
	alertMessage, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `alert-message`: %w", err)
	}
	slog.Debug("read `alert-message`", "alert-message", alertMessage)
	return &TLSAlertReceivedPayloadRecord{alertId, alertMessage}, nil
}

func (v *TLSAlertReceivedPayloadRecord) WriteTo(w wrpc.ByteWriter) error {
	slog.Debug("writing `alert-id`")
	if err := wrpc.WriteOption(v.AlertId, w, wrpc.WriteUint8); err != nil {
		return fmt.Errorf("failed to write `alert-id`: %w", err)
	}
	slog.Debug("writing `alert-message`")
	if err := wrpc.WriteOption(v.AlertMessage, w, wrpc.WriteString); err != nil {
		return fmt.Errorf("failed to write `alert-message`: %w", err)
	}
	return nil
}

type FieldSizePayloadRecord struct {
	FieldName *string
	FieldSize *uint32
}

func ReadFieldSizePayload(r wrpc.ByteReader) (*FieldSizePayloadRecord, error) {
	slog.Debug("reading `field-name`")
	fieldName, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `field-name`: %w", err)
	}
	slog.Debug("read `field-name`", "field-name", fieldName)
	slog.Debug("reading `field-size`")
	fieldSize, err := wrpc.ReadOption(r, wrpc.ReadUint32)
	if err != nil {
		return nil, fmt.Errorf("failed to read `field-size`: %w", err)
	}
	slog.Debug("read `field-size`", "field-size", fieldSize)
	return &FieldSizePayloadRecord{fieldName, fieldSize}, nil
}

func (v *FieldSizePayloadRecord) WriteTo(w wrpc.ByteWriter) error {
	slog.Debug("writing `field-name`")
	if err := wrpc.WriteOption(v.FieldName, w, wrpc.WriteString); err != nil {
		return fmt.Errorf("failed to write `field-name`: %w", err)
	}
	slog.Debug("writing `field-size`")
	if err := wrpc.WriteOption(v.FieldSize, w, wrpc.WriteUint32); err != nil {
		return fmt.Errorf("failed to write `field-size`: %w", err)
	}
	return nil
}

func SubscribeRequest(sub wrpc.Subscriber) (*RequestSubscription, error) {
	slog.Debug("subscribe for `body`")
	payloadBody := make(chan []byte)
	stopBody, err := sub.Subscribe(func(ctx context.Context, buf []byte) {
		payloadBody <- buf
	}, 0)
	if err != nil {
		return nil, err
	}

	slog.Debug("subscribe for `trailers`")
	payloadTrailers := make(chan []byte)
	stopTrailers, err := sub.Subscribe(func(ctx context.Context, buf []byte) {
		payloadTrailers <- buf
	}, 1)
	if err != nil {
		defer func() {
			if err := stopBody(); err != nil {
				slog.Error("failed to stop `body` subscription", "err", err)
			}
		}()
		return nil, err
	}

	return &RequestSubscription{
		payloadBody,
		payloadTrailers,
		stopBody,
		stopTrailers,
	}, nil
}

func ReadRequest(r wrpc.IndexReader, path ...uint32) (*RequestRecord, error) {
	slog.Debug("reading `body`")
	body, err := wrpc.ReadByteStream(r, append(path, 0)...)
	if err != nil {
		return nil, fmt.Errorf("failed to read `body`: %w", err)
	}
	slog.Debug("read `body`", "body", body)

	slog.Debug("reading `trailers`")
	trailers, err := wrpc.ReadFuture(r, func(r wrpc.ByteReader) ([]*wrpc.Tuple2[string, [][]byte], error) {
		return wrpc.ReadFlatOption(r, func(r wrpc.ByteReader) ([]*wrpc.Tuple2[string, [][]byte], error) {
			return wrpc.ReadList(r, func(r wrpc.ByteReader) (*wrpc.Tuple2[string, [][]byte], error) {
				return wrpc.ReadTuple2(r, wrpc.ReadString, func(r wrpc.ByteReader) ([][]byte, error) {
					return wrpc.ReadList(r, wrpc.ReadByteList)
				})
			})
		})
	}, append(path, 1)...)
	if err != nil {
		return nil, fmt.Errorf("failed to read `trailers`: %w", err)
	}
	slog.Debug("read `trailers`", "trailers", trailers)

	slog.Debug("reading `method`")
	method, err := ReadMethod(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `method`: %w", err)
	}
	slog.Debug("read `method`", "method", method)

	slog.Debug("reading `path-with-query`")
	pathWithQuery, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `path-with-query`: %w", err)
	}
	slog.Debug("read `path-with-query`", "path-with-query", *pathWithQuery)

	slog.Debug("reading `scheme`")
	scheme, err := wrpc.ReadFlatOption(r, ReadScheme)
	if err != nil {
		return nil, fmt.Errorf("failed to read `scheme`: %w", err)
	}
	slog.Debug("read `scheme`", "scheme", scheme)

	slog.Debug("reading `authority`")
	authority, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `authority`: %w", err)
	}
	slog.Debug("read `authority`", "authority", authority)

	slog.Debug("reading `headers`")
	headers, err := wrpc.ReadList(r, func(r wrpc.ByteReader) (*wrpc.Tuple2[string, [][]byte], error) {
		return wrpc.ReadTuple2(r, wrpc.ReadString, func(r wrpc.ByteReader) ([][]byte, error) {
			return wrpc.ReadList(r, wrpc.ReadByteList)
		})
	})
	if err != nil {
		return nil, fmt.Errorf("failed to read `headers`: %w", err)
	}
	slog.Debug("read `headers`", "headers", headers)

	return &RequestRecord{body, trailers, method, pathWithQuery, scheme, authority, headers}, nil
}

func ReadRequestOptions(r wrpc.ByteReader) (*RequestOptionsRecord, error) {
	slog.Debug("reading `connect-timeout`")
	connectTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `connect-timeout`: %w", err)
	}

	slog.Debug("reading `first-byte-timeout`")
	firstByteTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `first-byte-timeout`: %w", err)
	}

	slog.Debug("reading `between-byte-timeout`")
	betweenByteTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `between-byte-timeout`: %w", err)
	}

	return &RequestOptionsRecord{connectTimeout, firstByteTimeout, betweenByteTimeout}, nil
}

func (v *ResponseRecord) WriteToIndex(w wrpc.ByteWriter) (write func(wrpc.Index[wrpc.IndexWriter]) error, err error) {
	writes := map[uint32]func(wrpc.IndexWriter) error{}

	slog.Debug("writing `body`")
	if v.Body.Ready() {
		defer func() {
			body, ok := v.Body.(io.Closer)
			if ok {
				if cErr := body.Close(); cErr != nil {
					if err == nil {
						err = fmt.Errorf("failed to close ready byte stream: %w", cErr)
					} else {
						slog.Warn("failed to close ready byte stream", "err", cErr)
					}
				}
			}
		}()
		slog.Debug("writing byte stream `stream::ready` status byte")
		if err := w.WriteByte(1); err != nil {
			return nil, fmt.Errorf("failed to write `stream::ready` byte: %w", err)
		}
		slog.Debug("reading ready byte stream contents")
		var buf bytes.Buffer
		n, err := io.Copy(&buf, v.Body)
		if err != nil {
			return nil, fmt.Errorf("failed to read ready byte stream contents: %w", err)
		}
		slog.Debug("writing ready byte stream contents", "len", n)
		if err = wrpc.WriteByteList(buf.Bytes(), w); err != nil {
			return nil, fmt.Errorf("failed to write ready byte stream contents: %w", err)
		}
		return nil, nil
	} else {
		slog.Debug("writing byte stream `stream::pending` status byte")
		if err := w.WriteByte(0); err != nil {
			return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
		}
		writes[0] = func(w wrpc.IndexWriter) (err error) {
			defer func() {
				body, ok := v.Body.(io.Closer)
				if ok {
					if cErr := body.Close(); cErr != nil {
						if err == nil {
							err = fmt.Errorf("failed to close pending byte stream: %w", cErr)
						} else {
							slog.Warn("failed to close pending byte stream", "err", cErr)
						}
					}
				}
			}()
			chunk := make([]byte, 8096)
			for {
				var end bool
				slog.Debug("reading pending byte stream contents")
				n, err := v.Body.Read(chunk)
				if err == io.EOF {
					end = true
					slog.Debug("pending byte stream reached EOF")
				} else if err != nil {
					return fmt.Errorf("failed to read pending byte stream chunk: %w", err)
				}
				if n > math.MaxUint32 {
					return fmt.Errorf("pending byte stream chunk length of %d overflows a 32-bit integer", n)
				}
				slog.Debug("writing pending byte stream chunk length", "len", n)
				if err := wrpc.WriteUint32(uint32(n), w); err != nil {
					return fmt.Errorf("failed to write pending byte stream chunk length of %d: %w", n, err)
				}
				_, err = w.Write(chunk[:n])
				if err != nil {
					return fmt.Errorf("failed to write pending byte stream chunk contents: %w", err)
				}
				if end {
					if err := w.WriteByte(0); err != nil {
						return fmt.Errorf("failed to write pending byte stream end byte: %w", err)
					}
					return nil
				}
			}
		}
	}
	slog.Debug("writing `trailers`")
	if v.Trailers.Ready() {
		if err := w.WriteByte(1); err != nil {
			return nil, fmt.Errorf("failed to write `future::ready` byte: %w", err)
		}
		trailers, err := v.Trailers.Receive()
		if err != nil {
			return nil, fmt.Errorf("failed to receive `trailers`: %w", err)
		}
		if err := wrpc.WriteOption(wrpc.Slice(trailers), w, func(v []*wrpc.Tuple2[string, [][]byte], w wrpc.ByteWriter) error {
			return wrpc.WriteList(trailers, w, func(v *wrpc.Tuple2[string, [][]byte], w wrpc.ByteWriter) error {
				return v.WriteTo(w, wrpc.WriteString, func(v [][]byte, w wrpc.ByteWriter) error {
					return wrpc.WriteList(v, w, wrpc.WriteByteList)
				})
			})
		}); err != nil {
			return nil, fmt.Errorf("failed to write `trailers`: %w", err)
		}
	} else {
		if err := w.WriteByte(0); err != nil {
			return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
		}
		writes[1] = func(w wrpc.IndexWriter) error {
			trailers, err := v.Trailers.Receive()
			if err != nil {
				return fmt.Errorf("failed to receive `trailers`: %w", err)
			}
			if err := wrpc.WriteList(trailers, w, func(v *wrpc.Tuple2[string, [][]byte], w wrpc.ByteWriter) error {
				return v.WriteTo(w, wrpc.WriteString, func(v [][]byte, w wrpc.ByteWriter) error {
					return wrpc.WriteList(v, w, wrpc.WriteByteList)
				})
			}); err != nil {
				return fmt.Errorf("failed to write `trailers`: %w", err)
			}
			return nil
		}
	}
	slog.Debug("writing `status`")
	if err := wrpc.WriteUint16(v.Status, w); err != nil {
		return nil, fmt.Errorf("failed to write `status`: %w", err)
	}
	slog.Debug("writing `headers`")
	if err := wrpc.WriteList(v.Headers, w, func(v *wrpc.Tuple2[string, [][]byte], w wrpc.ByteWriter) error {
		return v.WriteTo(w, wrpc.WriteString, func(v [][]byte, w wrpc.ByteWriter) error {
			return wrpc.WriteList(v, w, wrpc.WriteByteList)
		})
	}); err != nil {
		return nil, fmt.Errorf("failed to write `headers`: %w", err)
	}
	if len(writes) > 0 {
		return func(w wrpc.Index[wrpc.IndexWriter]) error {
			var wg errgroup.Group
			for index, write := range writes {
				w, err := w.Index(index)
				if err != nil {
					return fmt.Errorf("failed to index writer: %w", err)
				}
				write := write
				wg.Go(func() error {
					return write(w)
				})
			}
			return wg.Wait()
		}, nil
	}
	return nil, nil
}
