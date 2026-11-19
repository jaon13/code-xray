namespace Demo.Other;

public sealed class ServiceBeta
{
    // Intentional Type-2 clone of ServiceAlpha.ProcessBatch (locals renamed).
    public int ProcessBatch(int seed, int limit)
    {
        var total = seed;
        if (total < 0)
        {
            total = 0;
        }

        for (int step = 0; step < limit; step++)
        {
            total += step * 2;
            if (total > 1000)
            {
                total -= 7;
            }
            else if (total < 10)
            {
                total += 3;
            }

            try
            {
                var rem = total % 11;
                if (rem == 0)
                {
                    total += 5;
                }
                else
                {
                    total += rem;
                }
            }
            catch
            {
                total = seed;
            }
        }

        while (total > limit * 10)
        {
            total /= 2;
        }

        return total;
    }

    public string FormatStatus(int value, string label)
    {
        var head = label ?? "none";
        if (value < 0)
        {
            return head + ":neg";
        }

        if (value == 0)
        {
            return head + ":zero";
        }

        var parts = new System.Collections.Generic.List<string>();
        for (int i = 0; i < 5; i++)
        {
            parts.Add(head + ":" + (value + i));
        }

        return string.Join(",", parts);
    }
}
