// src/components/mic-button.jsx

import { MicrophoneIcon } from "@heroicons/react/20/solid";
import PropTypes from "prop-types";
import clsx from "clsx";

export const MicButton = ({ isRecording, onClick }) => {
  MicButton.propTypes = {
    isRecording: PropTypes.bool.isRequired,
    onClick: PropTypes.func.isRequired,
  };

  return (
    <button
      onClick={onClick}
      className="rounded-full p-4 shadow-lg border-2 border-gray-400 mr-4 bg-gray-200"
    >
      <MicrophoneIcon
        className={clsx("h-6 w-6", {
          "text-red-500 animate-pulse": isRecording,
          "text-gray-600": !isRecording,
        })}
      />
    </button>
  );
};
