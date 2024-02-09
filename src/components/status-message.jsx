// src/components/status-message.jsx

import PropTypes from "prop-types";

export const StatusMessage = ({ message }) => {
  StatusMessage.propTypes = {
    message: PropTypes.string.isRequired,
  };

  return <div className="text-gray-600 dark:text-gray-300">{message}</div>;
};
