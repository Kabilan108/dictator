import { useTheme } from "@/lib/ThemeContext";
import * as SelectPrimitive from '@radix-ui/react-select';

interface SelectBoxProps {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
  disabled?: boolean;
  className?: string;
}

const SelectBox = ({
  value, onChange, options, disabled = false, className = ""
}: SelectBoxProps) => {
  const { colors } = useTheme();

  return (
    <SelectPrimitive.Root value={value} onValueChange={onChange} disabled={disabled}>
      <SelectPrimitive.Trigger
        className={`flex justify-between items-center w-full border rounded px-2 py-1 ${className}`}
        style={{
          background: disabled ? colors.mantle : colors.surface0,
          borderColor: disabled ? colors.mantle : colors.surface1,
          color: colors.text,
          opacity: disabled ? 0.7 : 1,
          cursor: disabled ? 'not-allowed' : 'pointer',
        }}
      >
        <SelectPrimitive.Value placeholder="Select an option" />
        <SelectPrimitive.Icon>
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <polyline points="6 9 12 15 18 9"></polyline>
          </svg>
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>

      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          position="popper"
          sideOffset={4}
          style={{
            backgroundColor: colors.surface0,
            color: colors.text,
            borderColor: colors.surface1,
          }}
          className="border rounded shadow-md z-50 overflow-hidden min-w-[var(--radix-select-trigger-width)]"
        >
          <SelectPrimitive.ScrollUpButton
            className="flex items-center justify-center h-6 text-xs bg-transparent hover:bg-opacity-10 hover:bg-current"
            style={{ color: colors.overlay }}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M18 15l-6-6-6 6" />
            </svg>
          </SelectPrimitive.ScrollUpButton>

          <SelectPrimitive.Viewport className="p-1 max-h-40">
            {options.map(option => (
              <SelectPrimitive.Item
                key={option.value}
                value={option.value}
                className="flex items-center px-2 py-1 rounded text-sm outline-none relative select-none"
                style={{
                  color: colors.text,
                }}
              >
                <div className="absolute inset-0 rounded-sm data-[highlighted]:bg-opacity-20 data-[highlighted]:bg-current data-[state=checked]:bg-opacity-10 data-[state=checked]:bg-current" />
                <SelectPrimitive.ItemText>{option.label}</SelectPrimitive.ItemText>
                {option.value === value && (
                  <span className="ml-auto">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M20 6L9 17l-5-5" />
                    </svg>
                  </span>
                )}
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>

          <SelectPrimitive.ScrollDownButton
            className="flex items-center justify-center h-6 text-xs bg-transparent hover:bg-opacity-10 hover:bg-current"
            style={{ color: colors.overlay }}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M6 9l6 6 6-6" />
            </svg>
          </SelectPrimitive.ScrollDownButton>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
};

export default SelectBox;
