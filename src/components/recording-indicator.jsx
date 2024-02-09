// src/components/recording-indicator.jsx

import PropTypes from "prop-types";

export const RecordingIndicator = ({ isRecording }) => {
  RecordingIndicator.propTypes = {
    isRecording: PropTypes.bool.isRequired,
  };

  const textColor = isRecording ? "text-red-500" : "text-gray-500";

  return (
    <div className={`flex items-center ${textColor}`}>
      <span className="h-3 w-3 bg-red-500 rounded-full mr-2 animate-pulse"></span>
      <span>Recording...</span>
    </div>
  );
};
