#!/home/muaddib/.cache/pypoetry/virtualenvs/distil-1_vY1VFM-py3.10/bin/python
#
# worker.py
#
# A program that reads from STDIN and executes commands.

from typing import Callable, Optional
from pathlib import Path
import time
import sys

from transformers import AutoModelForSpeechSeq2Seq, AutoProcessor, pipeline
from loguru import logger
import torch

logger.add(
    "worker.log",
    format="{time} {level} {message}",
    level="INFO",
    rotation="1 week",
    compression="zip",
)

MODEL = "distil-whisper/distil-small.en"


def check_cuda() -> bool:
    """Check if CUDA is available."""

    try:
        return torch.cuda.is_available()
    except Exception as e:
        logger.error(f"CUDA error: {e}")
        return False


def load_model() -> Optional[Callable]:
    """Load model onto GPU."""

    # TODO: rn this only handles short (<30s) clips efficiently
    # TODO: implement the chunking strategy in distil-whisper README

    try:
        device = "cuda:0" if check_cuda() else "cpu"
        torch_dtype = torch.float16 if check_cuda() else torch.float32

        model = AutoModelForSpeechSeq2Seq.from_pretrained(
            MODEL, torch_dtype=torch_dtype, low_cpu_mem_usage=True, use_safetensors=True
        )
        model.to(device)

        processor = AutoProcessor.from_pretrained(MODEL)

        pipe = pipeline(
            "automatic-speech-recognition",
            model=model,
            tokenizer=processor.tokenizer,
            feature_extractor=processor.feature_extractor,
            max_new_tokens=128,
            torch_dtype=torch_dtype,
            device=device,
        )
        return pipe
    except Exception as e:
        logger.error(f"Model loading error: {e}")
        return None


def transcribe(pipe, audiofile):
    """Transcribe audio file."""

    tic = time.time()
    try:
        result = pipe(audiofile)
        torch.cuda.empty_cache()
        return result["text"], time.time() - tic
    except Exception as e:
        logger.error(f"Error during transcription: {e}")
        return "[error] Transcription failed.", 0.0


def print_(*args, **kwargs):
    """Print with flush."""
    print(*args, **kwargs)
    sys.stdout.flush()


def print_transcript(transcript, duration):
    """Print transcript and duration"""
    print_(f"[transcript] {transcript}")


if __name__ == "__main__":
    pipe = load_model()

    print_("[ready]")

    try:
        for line in sys.stdin:
            if r"\transcribe" in line:
                cmd, audiofile = line.split()

                if Path(audiofile).exists():
                    transcript, duration = transcribe(pipe, audiofile)
                    print_transcript(transcript, duration)
                    sys.stdout.flush()
                else:
                    print_("[error] File does not exist.")

            elif r"\exit" in line:
                del pipe
                break

            else:
                print_("[error] Unknown command.")
    except KeyboardInterrupt:
        pass
    except Exception as e:
        logger.error(f"Unexpected error: {e}")

    print_("[bye]")
